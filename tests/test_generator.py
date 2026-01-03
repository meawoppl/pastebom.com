"""Generator tests for ibom-gist."""

import pytest
from pathlib import Path

from app.services.generator import HeadlessConfig, BomGenerator


def test_headless_config_defaults():
    """Test HeadlessConfig with default values."""
    config = HeadlessConfig()
    assert config.compression is True
    assert config.dark_mode is False
    assert config.show_pads is True
    assert config.show_silkscreen is True
    assert config.bom_view == "left-right"
    assert config.layer_view == "FB"


def test_headless_config_overrides():
    """Test HeadlessConfig with custom values."""
    config = HeadlessConfig(dark_mode=True, compression=False)
    assert config.dark_mode is True
    assert config.compression is False


def test_headless_config_get_html_config():
    """Test HTML config generation."""
    config = HeadlessConfig(dark_mode=True)
    html_config = config.get_html_config()
    assert "dark_mode" in html_config
    assert "true" in html_config.lower()


@pytest.fixture
def generator(tmp_path):
    """Create a generator with temp storage."""
    return BomGenerator(str(tmp_path / "boms"))


def test_generator_storage_path_created(generator):
    """Test that storage path is created."""
    assert Path(generator.storage_path).exists()


def test_generator_get_nonexistent(generator):
    """Test getting non-existent BOM."""
    result = generator.get("nonexistent-uuid")
    assert result is None


def test_generator_get_meta_nonexistent(generator):
    """Test getting metadata for non-existent BOM."""
    result = generator.get_meta("nonexistent-uuid")
    assert result is None
