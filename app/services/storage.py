"""S3 storage service for uploads and generated BOMs."""

import json
import logging
from typing import Optional

import boto3
from botocore.exceptions import ClientError

from ..config import settings

logger = logging.getLogger("ibom-gist.storage")


class S3Storage:
    """Handles S3 storage for uploads and generated BOMs."""

    def __init__(self):
        self.bucket = settings.S3_BUCKET
        self.region = settings.S3_REGION
        self.uploads_prefix = settings.S3_UPLOADS_PREFIX
        self.boms_prefix = settings.S3_BOMS_PREFIX

        # Create S3 client
        client_kwargs = {"region_name": self.region}
        if settings.S3_ENDPOINT_URL:
            client_kwargs["endpoint_url"] = settings.S3_ENDPOINT_URL

        self.s3 = boto3.client("s3", **client_kwargs)

    def _upload_key(self, bom_id: str, filename: str) -> str:
        """Generate S3 key for upload file."""
        return f"{self.uploads_prefix}/{bom_id}/{filename}"

    def _bom_key(self, bom_id: str) -> str:
        """Generate S3 key for generated BOM HTML."""
        return f"{self.boms_prefix}/{bom_id}.html"

    def _meta_key(self, bom_id: str) -> str:
        """Generate S3 key for BOM metadata."""
        return f"{self.boms_prefix}/{bom_id}.meta.json"

    def store_upload(self, bom_id: str, filename: str, content: bytes) -> str:
        """
        Store original upload file to S3 (private).

        Returns the S3 key.
        """
        key = self._upload_key(bom_id, filename)
        self.s3.put_object(
            Bucket=self.bucket,
            Key=key,
            Body=content,
            ContentType="application/octet-stream",
        )
        logger.info(f"Stored upload: s3://{self.bucket}/{key}")
        return key

    def store_bom(self, bom_id: str, html: str, meta: dict) -> str:
        """
        Store generated BOM HTML (public) and metadata (private) to S3.

        Returns the S3 key for the HTML file.
        """
        # Store HTML with public-read ACL
        html_key = self._bom_key(bom_id)
        self.s3.put_object(
            Bucket=self.bucket,
            Key=html_key,
            Body=html.encode("utf-8"),
            ContentType="text/html; charset=utf-8",
            ACL="public-read",
        )
        logger.info(f"Stored BOM: s3://{self.bucket}/{html_key}")

        # Store metadata (private)
        meta_key = self._meta_key(bom_id)
        self.s3.put_object(
            Bucket=self.bucket,
            Key=meta_key,
            Body=json.dumps(meta).encode("utf-8"),
            ContentType="application/json",
        )
        logger.info(f"Stored metadata: s3://{self.bucket}/{meta_key}")

        return html_key

    def get_bom_url(self, bom_id: str) -> str:
        """
        Get public URL for a BOM.

        Returns direct S3 URL for public bucket.
        """
        key = self._bom_key(bom_id)

        # For custom endpoints (LocalStack/MinIO), use endpoint URL
        if settings.S3_ENDPOINT_URL:
            return f"{settings.S3_ENDPOINT_URL}/{self.bucket}/{key}"

        # Standard S3 public URL
        return f"https://{self.bucket}.s3.{self.region}.amazonaws.com/{key}"

    def get_meta(self, bom_id: str) -> Optional[dict]:
        """Retrieve BOM metadata from S3."""
        key = self._meta_key(bom_id)
        try:
            response = self.s3.get_object(Bucket=self.bucket, Key=key)
            return json.loads(response["Body"].read().decode("utf-8"))
        except ClientError as e:
            if e.response["Error"]["Code"] == "NoSuchKey":
                return None
            raise

    def bom_exists(self, bom_id: str) -> bool:
        """Check if a BOM exists in S3."""
        key = self._bom_key(bom_id)
        try:
            self.s3.head_object(Bucket=self.bucket, Key=key)
            return True
        except ClientError as e:
            if e.response["Error"]["Code"] == "404":
                return False
            raise
