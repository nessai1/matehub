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
    pub endpoint: String,
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
    pub async fn new(
        endpoint: &str,
        region: &str,
        access_key: &str,
        secret_key: &str,
        bucket: &str,
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
        }
    }

    /// Create from env vars with a specific bucket env var name.
    pub async fn from_env(bucket_env: &str) -> Self {
        let endpoint =
            std::env::var("S3_ENDPOINT").unwrap_or_else(|_| "http://localhost:9000".into());
        let region = std::env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".into());
        let access_key = std::env::var("S3_ACCESS_KEY_ID").expect("S3_ACCESS_KEY_ID required");
        let secret_key =
            std::env::var("S3_SECRET_ACCESS_KEY").expect("S3_SECRET_ACCESS_KEY required");
        let bucket = std::env::var(bucket_env).unwrap_or_else(|_| bucket_env.to_string());

        Self::new(&endpoint, &region, &access_key, &secret_key, &bucket).await
    }

    /// Get the public URL for a given S3 key.
    pub fn public_url(&self, key: &str) -> String {
        format!("{}/{}/{}", self.endpoint, self.bucket, key)
    }

    /// Extract S3 key from a URL previously produced by `upload`.
    pub fn key_from_url(&self, url: &str) -> Option<String> {
        let prefix = format!("{}/{}/", self.endpoint, self.bucket);
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
