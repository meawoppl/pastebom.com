"""Pydantic models for BOM API."""

from typing import Optional
from pydantic import BaseModel


class BomResponse(BaseModel):
    """Response returned after successful BOM upload."""
    id: str
    url: str
    filename: str
    components: int
    created_at: str
    expires_at: Optional[str] = None


class BomMeta(BaseModel):
    """Metadata about a stored BOM."""
    id: str
    filename: str
    components: int
    file_size: int
    created_at: str
    format: Optional[str] = None


class ErrorResponse(BaseModel):
    """Error response format."""
    error: str
    message: str
    supported: Optional[list[str]] = None
