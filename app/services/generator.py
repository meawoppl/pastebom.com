"""BOM Generator Service - wraps InteractiveHtmlBom for headless use."""

import json
import logging
import os
import sys
import tempfile
import uuid
from datetime import datetime
from pathlib import Path
from typing import Optional

from .storage import S3Storage

# Set NO_DISPLAY and CLI_MODE env vars to skip wx imports in InteractiveHtmlBom
os.environ["INTERACTIVE_HTML_BOM_NO_DISPLAY"] = "1"
os.environ["INTERACTIVE_HTML_BOM_CLI_MODE"] = "1"
IBOM_ROOT = Path(__file__).parent.parent.parent / "InteractiveHtmlBom"
IBOM_PACKAGE = IBOM_ROOT / "InteractiveHtmlBom"
# Add parent of InteractiveHtmlBom to sys.path so imports work correctly
IBOM_PARENT = IBOM_ROOT.parent
if str(IBOM_PARENT) not in sys.path:
    sys.path.insert(0, str(IBOM_PARENT))


class HeadlessLogger:
    """Logger that doesn't depend on wx."""

    def __init__(self):
        self.logger = logging.getLogger("ibom-gist")

    def info(self, *args):
        self.logger.info(*args)

    def error(self, msg):
        self.logger.error(msg)

    def warn(self, msg):
        self.logger.warning(msg)


class HeadlessConfig:
    """Config that doesn't depend on wx.FileConfig."""

    # Class-level constants from original Config
    bom_view_choices = ["bom-only", "left-right", "top-bottom"]
    layer_view_choices = ["F", "FB", "B"]
    default_sort_order = [
        "C", "R", "L", "D", "U", "Y", "X", "F", "SW", "A",
        "~",
        "HS", "CNN", "J", "P", "NT", "MH",
    ]
    highlight_pin1_choices = ["none", "all", "selected"]
    default_checkboxes = ["Sourced", "Placed"]
    html_config_fields = [
        "dark_mode", "show_pads", "show_fabrication", "show_silkscreen",
        "highlight_pin1", "redraw_on_drag", "board_rotation", "checkboxes",
        "bom_view", "layer_view", "offset_back_rotation",
        "kicad_text_formatting", "mark_when_checked"
    ]

    def __init__(self, version: str = "2.10.0", **kwargs):
        self.version = version

        # HTML defaults
        self.dark_mode = kwargs.get("dark_mode", False)
        self.show_pads = kwargs.get("show_pads", True)
        self.show_fabrication = kwargs.get("show_fabrication", False)
        self.show_silkscreen = kwargs.get("show_silkscreen", True)
        self.redraw_on_drag = kwargs.get("redraw_on_drag", True)
        self.highlight_pin1 = kwargs.get("highlight_pin1", "none")
        self.board_rotation = kwargs.get("board_rotation", 0)
        self.offset_back_rotation = kwargs.get("offset_back_rotation", False)
        self.checkboxes = kwargs.get("checkboxes", "Sourced,Placed")
        self.mark_when_checked = kwargs.get("mark_when_checked", "")
        self.bom_view = kwargs.get("bom_view", "left-right")
        self.layer_view = kwargs.get("layer_view", "FB")
        self.compression = kwargs.get("compression", True)
        self.open_browser = False  # Never for web service
        self.kicad_text_formatting = kwargs.get("kicad_text_formatting", True)

        # General
        self.bom_dest_dir = ""  # We handle output ourselves
        self.bom_name_format = "ibom"
        self.component_sort_order = kwargs.get(
            "component_sort_order", self.default_sort_order.copy()
        )
        self.component_blacklist = kwargs.get("component_blacklist", [])
        self.blacklist_virtual = kwargs.get("blacklist_virtual", True)
        self.blacklist_empty_val = kwargs.get("blacklist_empty_val", False)
        self.include_tracks = kwargs.get("include_tracks", False)
        self.include_nets = kwargs.get("include_nets", False)

        # Fields
        self.extra_data_file = None
        self.netlist_initial_directory = ""
        self.show_fields = kwargs.get("show_fields", ["Value", "Footprint"])
        self.group_fields = kwargs.get("group_fields", ["Value", "Footprint"])
        self.normalize_field_case = kwargs.get("normalize_field_case", False)
        self.board_variant_field = ""
        self.board_variant_whitelist = []
        self.board_variant_blacklist = []
        self.dnp_field = kwargs.get("dnp_field", "")

    def get_html_config(self):
        d = {f: getattr(self, f) for f in self.html_config_fields}
        d["fields"] = self.show_fields
        return json.dumps(d)


class BomGenerator:
    """Generates interactive HTML BOMs from PCB files."""

    def __init__(self, storage: S3Storage):
        self.storage = storage
        self.logger = HeadlessLogger()
        self._ibom_version = None

    def _get_ibom_version(self) -> str:
        """Get version from InteractiveHtmlBom."""
        if self._ibom_version is None:
            try:
                from InteractiveHtmlBom.InteractiveHtmlBom.version import version
                self._ibom_version = version
            except ImportError:
                self._ibom_version = "2.10.0"
        return self._ibom_version

    def _get_web_file_content(self, file_name: str) -> str:
        """Load content from InteractiveHtmlBom/web directory."""
        path = IBOM_PACKAGE / "web" / file_name
        if not path.exists():
            return ""
        return path.read_text(encoding="utf-8")

    def generate(
        self,
        file_content: bytes,
        filename: str,
        config_overrides: dict = None
    ) -> dict:
        """
        Generate an interactive HTML BOM from uploaded file.

        Args:
            file_content: Raw bytes of the uploaded file
            filename: Original filename (used to detect format)
            config_overrides: Optional config overrides from user

        Returns:
            dict with 'id', 'filename', 'components'

        Raises:
            ValueError: If file format unsupported
            RuntimeError: If parsing or generation fails
        """
        # Import from InteractiveHtmlBom (parent added to sys.path above)
        from InteractiveHtmlBom.InteractiveHtmlBom.ecad import get_parser_by_extension
        from InteractiveHtmlBom.InteractiveHtmlBom.core.ibom import generate_bom, round_floats
        from InteractiveHtmlBom.InteractiveHtmlBom.core.lzstring import LZString

        config = HeadlessConfig(
            version=self._get_ibom_version(),
            **(config_overrides or {})
        )

        # Generate UUID upfront so we can store upload with same ID
        bom_id = str(uuid.uuid4())

        # Store original upload to S3 for durability
        self.storage.store_upload(bom_id, filename, file_content)

        # Write to temp file (parsers expect file path)
        ext = os.path.splitext(filename)[1]
        with tempfile.NamedTemporaryFile(suffix=ext, delete=False) as tmp:
            tmp.write(file_content)
            tmp_path = tmp.name

        try:
            # Get appropriate parser
            parser = get_parser_by_extension(tmp_path, config, self.logger)
            if parser is None:
                raise ValueError(f"Unsupported file format: {ext}")

            # Parse PCB data
            pcbdata, components = parser.parse()
            if not pcbdata or not components:
                raise RuntimeError("Failed to parse PCB file")

            # Generate BOM
            pcbdata["bom"] = generate_bom(components, config)
            pcbdata["ibom_version"] = config.version

            # Generate HTML
            html_content = self._render_html(pcbdata, config, LZString, round_floats)

            # Store to S3
            self._store(bom_id, html_content, filename, len(components))

            return {
                "id": bom_id,
                "filename": filename,
                "components": len(components),
            }

        finally:
            os.unlink(tmp_path)

    def _render_html(self, pcbdata: dict, config: HeadlessConfig, LZString, round_floats) -> str:
        """Render the self-contained HTML file."""

        # Generate pcbdata JavaScript
        pcbdata_str = json.dumps(round_floats(pcbdata, 6))
        if config.compression:
            pcbdata_str = json.dumps(
                LZString().compress_to_base64(pcbdata_str)
            )
            pcbdata_js = f"var pcbdata = JSON.parse(LZString.decompressFromBase64({pcbdata_str}))"
        else:
            pcbdata_js = f"var pcbdata = {pcbdata_str}"

        config_js = "var config = " + config.get_html_config()

        # Build HTML from template
        html = self._get_web_file_content("ibom.html")
        html = html.replace("///CSS///", self._get_web_file_content("ibom.css"))
        html = html.replace("///USERCSS///", self._get_web_file_content("user.css"))
        html = html.replace("///SPLITJS///", self._get_web_file_content("split.js"))
        html = html.replace(
            "///LZ-STRING///",
            self._get_web_file_content("lz-string.js") if config.compression else ""
        )
        html = html.replace(
            "///POINTER_EVENTS_POLYFILL///",
            self._get_web_file_content("pep.js")
        )
        html = html.replace("///CONFIG///", config_js)
        html = html.replace("///UTILJS///", self._get_web_file_content("util.js"))
        html = html.replace("///RENDERJS///", self._get_web_file_content("render.js"))
        html = html.replace("///TABLEUTILJS///", self._get_web_file_content("table-util.js"))
        html = html.replace("///IBOMJS///", self._get_web_file_content("ibom.js"))
        html = html.replace("///USERJS///", self._get_web_file_content("user.js"))
        html = html.replace("///USERHEADER///", self._get_web_file_content("userheader.html"))
        html = html.replace("///USERFOOTER///", self._get_web_file_content("userfooter.html"))
        html = html.replace("///PCBDATA///", pcbdata_js)

        return html

    def _store(
        self,
        bom_id: str,
        html_content: str,
        filename: str,
        components: int
    ) -> None:
        """Store the generated HTML and metadata to S3."""
        meta = {
            "id": bom_id,
            "filename": filename,
            "components": components,
            "file_size": len(html_content.encode("utf-8")),
            "created_at": datetime.utcnow().isoformat() + "Z",
        }
        self.storage.store_bom(bom_id, html_content, meta)

    def get_bom_url(self, bom_id: str) -> Optional[str]:
        """Get public URL for a BOM."""
        if not self.storage.bom_exists(bom_id):
            return None
        return self.storage.get_bom_url(bom_id)

    def get_meta(self, bom_id: str) -> Optional[dict]:
        """Retrieve metadata from S3."""
        return self.storage.get_meta(bom_id)
