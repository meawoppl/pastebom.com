"""App configuration from environment variables."""

from typing import Optional
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_file_encoding="utf-8")

    MAX_UPLOAD_SIZE_MB: int = 50
    BOM_EXPIRY_DAYS: int = 0
    BASE_URL: str = "http://localhost:8080"
    LOG_LEVEL: str = "info"

    # S3 storage configuration
    S3_BUCKET: str  # Required - bucket name
    S3_REGION: str = "us-east-1"
    S3_ENDPOINT_URL: Optional[str] = None  # For LocalStack/MinIO testing
    S3_UPLOADS_PREFIX: str = "uploads"
    S3_BOMS_PREFIX: str = "boms"

    # BOM defaults
    DEFAULT_DARK_MODE: bool = False
    DEFAULT_COMPRESSION: bool = True


settings = Settings()
