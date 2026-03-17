use std::path::PathBuf;

pub struct ObjectInfo {
    pub key: String,
    pub last_modified: chrono::DateTime<chrono::Utc>,
}

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

    pub async fn list_objects(&self, prefix: &str) -> Result<Vec<ObjectInfo>, S3Error> {
        match &self.backend {
            StorageBackend::S3 {
                client,
                bucket,
                prefix: s3_prefix,
            } => {
                let full_prefix = s3_key(s3_prefix, prefix);
                let mut objects = Vec::new();
                let mut continuation_token: Option<String> = None;
                loop {
                    let mut req = client.list_objects_v2().bucket(bucket).prefix(&full_prefix);
                    if let Some(token) = &continuation_token {
                        req = req.continuation_token(token);
                    }
                    let resp = req.send().await.map_err(|e| S3Error(e.to_string()))?;
                    for obj in resp.contents() {
                        if let Some(key) = obj.key() {
                            let logical_key = if s3_prefix.is_empty() {
                                key.to_string()
                            } else {
                                let stripped = format!("{}/", s3_prefix.trim_end_matches('/'));
                                key.strip_prefix(&stripped).unwrap_or(key).to_string()
                            };
                            let last_modified = obj
                                .last_modified()
                                .and_then(|t| {
                                    chrono::DateTime::from_timestamp(t.secs(), t.subsec_nanos())
                                })
                                .unwrap_or_else(chrono::Utc::now);
                            objects.push(ObjectInfo {
                                key: logical_key,
                                last_modified,
                            });
                        }
                    }
                    if resp.is_truncated() == Some(true) {
                        continuation_token = resp.next_continuation_token().map(|s| s.to_string());
                    } else {
                        break;
                    }
                }
                Ok(objects)
            }
            StorageBackend::Filesystem { root } => {
                let dir = root.join(prefix);
                let mut objects = Vec::new();
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() {
                            let key = format!(
                                "{}/{}",
                                prefix.trim_end_matches('/'),
                                entry.file_name().to_string_lossy()
                            );
                            let last_modified = entry
                                .metadata()
                                .and_then(|m| m.modified())
                                .map(chrono::DateTime::<chrono::Utc>::from)
                                .unwrap_or_else(|_| chrono::Utc::now());
                            objects.push(ObjectInfo { key, last_modified });
                        }
                    }
                }
                Ok(objects)
            }
        }
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
