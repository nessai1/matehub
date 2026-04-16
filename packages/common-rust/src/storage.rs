use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Region};

pub struct S3Storage {
    pub client: Client,
    pub bucket: String,
    pub endpoint: String,
}

impl S3Storage {
    /// Create from explicit parameters (bucket name passed in, not from env).
    pub async fn new(endpoint: &str, region: &str, access_key: &str, secret_key: &str, bucket: &str) -> Self {
        let creds = Credentials::new(access_key, secret_key, None, None, "env");

        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .endpoint_url(endpoint)
            .credentials_provider(creds)
            .force_path_style(true)
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

    /// Upload bytes and return the URL.
    pub async fn upload(
        &self,
        key: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<String, aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::put_object::PutObjectError>>
    {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(body.into())
            .send()
            .await?;

        let url = format!("{}/{}/{}", self.endpoint, self.bucket, key);
        Ok(url)
    }
}
