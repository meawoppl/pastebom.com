"""App configuration from environment variables."""

from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_file_encoding="utf-8")

    STORAGE_PATH: str = "./data/boms"
    MAX_UPLOAD_SIZE_MB: int = 50
    BOM_EXPIRY_DAYS: int = 0
    BASE_URL: str = "http://localhost:8080"
    LOG_LEVEL: str = "info"

    # BOM defaults
    DEFAULT_DARK_MODE: bool = False
    DEFAULT_COMPRESSION: bool = True


settings = Settings()
