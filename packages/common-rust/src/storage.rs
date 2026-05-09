use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{
    BehaviorVersion, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use thiserror::Error;

/// S3 multipart minimum part size for non-final parts (5 MiB).
/// We use 8 MiB to reduce request count while staying well under AWS's 10,000 part cap.
const PART_SIZE: usize = 8 * 1024 * 1024;

pub struct S3Storage {
    pub client: Client,
    pub bucket: String,
    /// Internal-network endpoint the SDK speaks to (`http://minio:9000` on
    /// box, `https://storage.yandexcloud.net` on YC, etc.). Used only for
    /// API calls.
    pub endpoint: String,
    /// Base URL used to construct **client-facing** object URLs. Defaults
    /// to `endpoint`, which is correct for any deployment where the bucket
    /// is reachable from the browser at the same hostname the backend
    /// uses (AWS S3, public YC storage). On a box deploy that's not
    /// true: the SDK talks to `http://minio:9000` over the compose
    /// network, but the browser can only reach the bucket through Caddy
    /// at `https://<domain>/s3`. Setting `S3_PUBLIC_URL` env to that
    /// public path makes `public_url()` and `key_from_url()` round-trip
    /// the right thing to clients.
    pub public_url_base: String,
}

#[derive(Debug, Error)]
pub enum UploadError {
    #[error("upload exceeds max size ({max} bytes)")]
    TooLarge { max: usize },
    #[error("upload stream is empty")]
    Empty,
    #[error("stream read error: {0}")]
    Stream(String),
    #[error("S3 error: {0}")]
    S3(String),
}

impl S3Storage {
    /// Create from explicit parameters (bucket name passed in, not from env).
    /// `public_url_base` is what client-facing URLs are built from; pass the
    /// same value as `endpoint` when there's no separate public origin.
    pub async fn new(
        endpoint: &str,
        region: &str,
        access_key: &str,
        secret_key: &str,
        bucket: &str,
        public_url_base: &str,
    ) -> Self {
        let creds = Credentials::new(access_key, secret_key, None, None, "env");

        // aws-sdk-s3 1.40+ defaults to CRC32 flexible-checksums on every request,
        // which sends `x-amz-sdk-checksum-algorithm: CRC32` plus a chunked-trailer
        // body. MinIO (and quite a few S3-compatible stores) reject this on
        // multipart `UploadPart` calls — the symptom is exactly what we hit:
        // `PutObject` works (one shot, single-part), `UploadPart` 500s after the
        // first ~part-size of bytes. `WhenRequired` falls back to "only when the
        // operation needs it", which lines up with what MinIO accepts.
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .endpoint_url(endpoint)
            .credentials_provider(creds)
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();

        let client = Client::from_conf(config);

        tracing::info!(%endpoint, %bucket, "S3 storage initialized");

        Self {
            client,
            bucket: bucket.to_string(),
            endpoint: endpoint.to_string(),
            public_url_base: public_url_base.to_string(),
        }
    }

    /// Create from env vars with a specific bucket env var name.
    ///
    /// `S3_PUBLIC_URL` is read with a fallback to `S3_ENDPOINT`. On AWS /
    /// public-YC the two are identical; on box deploys the public URL
    /// points at the Caddy-proxied path and the endpoint stays inside the
    /// compose network.
    pub async fn from_env(bucket_env: &str) -> Self {
        let endpoint =
            std::env::var("S3_ENDPOINT").unwrap_or_else(|_| "http://localhost:9000".into());
        let public_url_base = std::env::var("S3_PUBLIC_URL").unwrap_or_else(|_| endpoint.clone());
        let region = std::env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".into());
        let access_key = std::env::var("S3_ACCESS_KEY_ID").expect("S3_ACCESS_KEY_ID required");
        let secret_key =
            std::env::var("S3_SECRET_ACCESS_KEY").expect("S3_SECRET_ACCESS_KEY required");
        let bucket = std::env::var(bucket_env).unwrap_or_else(|_| bucket_env.to_string());

        Self::new(
            &endpoint,
            &region,
            &access_key,
            &secret_key,
            &bucket,
            &public_url_base,
        )
        .await
    }

    /// Get the public URL for a given S3 key.
    pub fn public_url(&self, key: &str) -> String {
        format!("{}/{}/{}", self.public_url_base, self.bucket, key)
    }

    /// Extract S3 key from a URL previously produced by `upload`.
    pub fn key_from_url(&self, url: &str) -> Option<String> {
        let prefix = format!("{}/{}/", self.public_url_base, self.bucket);
        url.strip_prefix(&prefix).map(|s| s.to_string())
    }

    /// Upload fully-buffered bytes. For large objects prefer `upload_stream`.
    pub async fn upload(
        &self,
        key: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<
        String,
        aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::put_object::PutObjectError>,
    > {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(body.into())
            .send()
            .await?;

        Ok(self.public_url(key))
    }

    /// Stream bytes into S3 without buffering the full object in memory.
    ///
    /// Behavior:
    /// - Accumulates up to `PART_SIZE` (8 MiB) before committing to multipart.
    /// - If the whole stream fits in one part → single `PutObject` (cheaper, fewer RTTs).
    /// - Otherwise → S3 multipart upload, flushing 8 MiB parts as they fill.
    /// - Total size is checked against `max_size` every chunk; overage aborts the upload.
    ///
    /// Returns (public_url, total_bytes_written).
    pub async fn upload_stream<S, E>(
        &self,
        key: &str,
        content_type: &str,
        mut stream: S,
        max_size: usize,
    ) -> Result<(String, usize), UploadError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let mut buf = BytesMut::with_capacity(PART_SIZE);
        let mut total: usize = 0;

        // Phase 1: buffer up to PART_SIZE so we can decide single-part vs multipart.
        while buf.len() < PART_SIZE {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    total += chunk.len();
                    if total > max_size {
                        return Err(UploadError::TooLarge { max: max_size });
                    }
                    buf.put(chunk);
                }
                Some(Err(e)) => return Err(UploadError::Stream(e.to_string())),
                None => break,
            }
        }

        // Small-file fast path: fits in a single PutObject.
        if buf.len() < PART_SIZE {
            if buf.is_empty() {
                return Err(UploadError::Empty);
            }
            let body = buf.freeze();
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .content_type(content_type)
                .body(ByteStream::from(body))
                .send()
                .await
                .map_err(|e| UploadError::S3(format!("put_object: {e}")))?;
            return Ok((self.public_url(key), total));
        }

        // Large-file path: S3 multipart upload.
        let create = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| UploadError::S3(format!("create_multipart_upload: {e}")))?;

        let upload_id = create
            .upload_id()
            .ok_or_else(|| UploadError::S3("no upload_id returned".into()))?
            .to_string();

        // Wrap the rest so any failure triggers an abort.
        let result = self
            .run_multipart(key, content_type, &upload_id, buf, stream, total, max_size)
            .await;

        if let Err(ref err) = result {
            tracing::warn!(%key, %upload_id, "aborting multipart upload: {err}");
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(&upload_id)
                .send()
                .await;
        }
        let _ = content_type; // suppress unused after move; kept for symmetry
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_multipart<S, E>(
        &self,
        key: &str,
        _content_type: &str,
        upload_id: &str,
        mut buf: BytesMut,
        mut stream: S,
        mut total: usize,
        max_size: usize,
    ) -> Result<(String, usize), UploadError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let mut parts: Vec<CompletedPart> = Vec::new();
        let mut part_number: i32 = 1;

        // Flush the first full part accumulated by the caller.
        while buf.len() >= PART_SIZE {
            let part = buf.split_to(PART_SIZE).freeze();
            let etag = self.put_part(key, upload_id, part_number, part).await?;
            parts.push(
                CompletedPart::builder()
                    .part_number(part_number)
                    .e_tag(etag)
                    .build(),
            );
            part_number += 1;
        }

        // Drain the rest of the stream.
        while let Some(next) = stream.next().await {
            let chunk = next.map_err(|e| UploadError::Stream(e.to_string()))?;
            total += chunk.len();
            if total > max_size {
                return Err(UploadError::TooLarge { max: max_size });
            }
            buf.put(chunk);
            while buf.len() >= PART_SIZE {
                let part = buf.split_to(PART_SIZE).freeze();
                let etag = self.put_part(key, upload_id, part_number, part).await?;
                parts.push(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(etag)
                        .build(),
                );
                part_number += 1;
            }
        }

        // Trailing part (any size, including < PART_SIZE — S3 allows the last part to be smaller).
        if !buf.is_empty() {
            let part = buf.freeze();
            let etag = self.put_part(key, upload_id, part_number, part).await?;
            parts.push(
                CompletedPart::builder()
                    .part_number(part_number)
                    .e_tag(etag)
                    .build(),
            );
        }

        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
            .map_err(|e| UploadError::S3(format!("complete_multipart_upload: {e}")))?;

        Ok((self.public_url(key), total))
    }

    /// Construct an `S3Storage` shape (no SDK calls) for unit-test
    /// assertions on URL helpers. Avoids `new()` because that hits the
    /// SDK config builder.
    #[cfg(test)]
    fn for_url_tests(endpoint: &str, bucket: &str, public_url_base: &str) -> Self {
        // The client value is never touched in URL helpers. We construct
        // a config-only client to satisfy the field type.
        let creds = Credentials::new("test", "test", None, None, "test");
        let conf = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(endpoint)
            .credentials_provider(creds)
            .force_path_style(true)
            .build();
        Self {
            client: Client::from_conf(conf),
            bucket: bucket.to_string(),
            endpoint: endpoint.to_string(),
            public_url_base: public_url_base.to_string(),
        }
    }

    async fn put_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: i32,
        body: Bytes,
    ) -> Result<String, UploadError> {
        let resp = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number)
            .body(ByteStream::from(body))
            .send()
            .await
            .map_err(|e| UploadError::S3(format!("upload_part #{part_number}: {e}")))?;

        resp.e_tag()
            .map(|s| s.to_string())
            .ok_or_else(|| UploadError::S3(format!("upload_part #{part_number}: no etag")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_uses_public_base_when_separate_from_endpoint() {
        // Box deploy shape: SDK talks to compose-internal `minio:9000`,
        // but client-facing URLs go through `https://demo/s3` reverse-
        // proxied by Caddy. `public_url` MUST surface the public form,
        // otherwise browsers get an unresolvable hostname.
        let s = S3Storage::for_url_tests(
            "http://minio:9000",
            "chat-media",
            "https://demo.matehub.io/s3",
        );
        assert_eq!(
            s.public_url("123/file.JPG"),
            "https://demo.matehub.io/s3/chat-media/123/file.JPG"
        );
    }

    #[test]
    fn public_url_falls_back_to_endpoint_when_base_equals_endpoint() {
        // AWS / public-YC shape: endpoint is already public. From `from_env`
        // we fall back to endpoint when `S3_PUBLIC_URL` is unset, so the
        // resulting URL must match what the legacy code produced.
        let s = S3Storage::for_url_tests(
            "https://storage.yandexcloud.net",
            "matehub-prod",
            "https://storage.yandexcloud.net",
        );
        assert_eq!(
            s.public_url("123/file.JPG"),
            "https://storage.yandexcloud.net/matehub-prod/123/file.JPG"
        );
    }

    #[test]
    fn key_from_url_strips_public_base_not_endpoint() {
        // Round-trip property: a URL produced by `public_url` must be
        // strippable by `key_from_url` to recover the original key,
        // *regardless of whether the public base differs from the
        // endpoint*. The previous implementation used `endpoint`, which
        // would silently drop every key once the box flips to a public
        // base.
        let s = S3Storage::for_url_tests(
            "http://minio:9000",
            "chat-media",
            "https://demo.matehub.io/s3",
        );
        let url = s.public_url("foo/bar.bin");
        assert_eq!(s.key_from_url(&url).as_deref(), Some("foo/bar.bin"));
    }
}
