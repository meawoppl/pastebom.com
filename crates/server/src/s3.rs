use aws_sdk_s3::Client;

#[derive(Clone)]
pub struct S3Client {
    client: Client,
    bucket: String,
    prefix: String,
}

#[derive(Debug)]
pub struct S3Error(pub String);

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for S3Error {}

impl S3Client {
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = Client::new(&config);
        let bucket = std::env::var("S3_BUCKET").unwrap_or_else(|_| "pastebom-uploads".to_string());
        let prefix = std::env::var("S3_PREFIX").unwrap_or_default();
        Self {
            client,
            bucket,
            prefix,
        }
    }

    fn key(&self, path: &str) -> String {
        if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{}", self.prefix.trim_end_matches('/'), path)
        }
    }

    pub async fn put_object(
        &self,
        path: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<(), S3Error> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .body(body.into())
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| S3Error(e.to_string()))?;
        Ok(())
    }

    pub async fn put_failed(&self, filename: &str, body: Vec<u8>) -> Result<(), S3Error> {
        let path = format!("failed/{filename}");
        self.put_object(&path, body, "application/octet-stream")
            .await
    }

    pub async fn get_object(&self, path: &str) -> Result<Vec<u8>, S3Error> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
            .map_err(|e| S3Error(e.to_string()))?;
        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| S3Error(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}
