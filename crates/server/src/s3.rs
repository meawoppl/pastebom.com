use std::path::PathBuf;

#[derive(Clone)]
pub struct S3Client {
    backend: StorageBackend,
}

#[derive(Clone)]
enum StorageBackend {
    S3 {
        client: aws_sdk_s3::Client,
        bucket: String,
        prefix: String,
    },
    Filesystem {
        root: PathBuf,
    },
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
        if let Ok(bucket) = std::env::var("S3_BUCKET") {
            let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let client = aws_sdk_s3::Client::new(&config);
            let prefix = std::env::var("S3_PREFIX").unwrap_or_default();
            tracing::info!("Using S3 storage: bucket={bucket}");
            Self {
                backend: StorageBackend::S3 {
                    client,
                    bucket,
                    prefix,
                },
            }
        } else {
            let root = std::env::var("STORAGE_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("./data"));
            std::fs::create_dir_all(&root).expect("Failed to create storage directory");
            tracing::info!("Using filesystem storage: {}", root.display());
            Self {
                backend: StorageBackend::Filesystem { root },
            }
        }
    }

    pub async fn put_object(
        &self,
        path: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<(), S3Error> {
        match &self.backend {
            StorageBackend::S3 {
                client,
                bucket,
                prefix,
            } => {
                let key = s3_key(prefix, path);
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(body.into())
                    .content_type(content_type)
                    .send()
                    .await
                    .map_err(|e| S3Error(e.to_string()))?;
                Ok(())
            }
            StorageBackend::Filesystem { root } => {
                let file_path = root.join(path);
                if let Some(parent) = file_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| S3Error(format!("mkdir failed: {e}")))?;
                }
                std::fs::write(&file_path, &body)
                    .map_err(|e| S3Error(format!("write failed: {e}")))?;
                Ok(())
            }
        }
    }

    pub async fn put_failed(&self, filename: &str, body: Vec<u8>) -> Result<(), S3Error> {
        let path = format!("failed/{filename}");
        self.put_object(&path, body, "application/octet-stream")
            .await
    }

    pub async fn get_object(&self, path: &str) -> Result<Vec<u8>, S3Error> {
        match &self.backend {
            StorageBackend::S3 {
                client,
                bucket,
                prefix,
            } => {
                let key = s3_key(prefix, path);
                let resp = client
                    .get_object()
                    .bucket(bucket)
                    .key(key)
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
            StorageBackend::Filesystem { root } => {
                let file_path = root.join(path);
                std::fs::read(&file_path).map_err(|e| S3Error(format!("read failed: {e}")))
            }
        }
    }
}

fn s3_key(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.to_string()
    } else {
        format!("{}/{}", prefix.trim_end_matches('/'), path)
    }
}
