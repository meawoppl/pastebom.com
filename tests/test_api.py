"""API tests for ibom-gist."""

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def client():
    """Create test client."""
    from app.main import app
    return TestClient(app)


def test_health(client):
    """Test health endpoint."""
    response = client.get("/health")
    assert response.status_code == 200
    assert response.json()["status"] == "ok"


def test_index(client):
    """Test index page."""
    response = client.get("/")
    assert response.status_code == 200
    assert "ibom-gist" in response.text


def test_upload_no_file(client):
    """Test upload without file."""
    response = client.post("/upload")
    assert response.status_code == 422


def test_upload_unsupported_format(client):
    """Test upload with unsupported format."""
    response = client.post(
        "/upload",
        files={"file": ("test.xyz", b"content", "application/octet-stream")}
    )
    assert response.status_code == 400
    assert response.json()["detail"]["error"] == "unsupported_format"


def test_view_nonexistent(client):
    """Test viewing non-existent BOM."""
    response = client.get("/b/nonexistent-uuid")
    assert response.status_code == 404


def test_meta_nonexistent(client):
    """Test getting metadata for non-existent BOM."""
    response = client.get("/b/nonexistent-uuid/meta")
    assert response.status_code == 404
