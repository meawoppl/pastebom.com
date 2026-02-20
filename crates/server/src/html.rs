use pcb_extract::types::PcbData;

/// Generate a self-contained HTML page that renders the interactive BOM viewer.
///
/// For the transitional approach, this embeds the pcbdata JSON into the
/// existing InteractiveHtmlBom JavaScript viewer template. The template
/// is stored as a const string and the pcbdata JSON is injected inline.
pub fn generate_html(pcb_data: &PcbData, title: &str) -> Result<String, serde_json::Error> {
    let json = serde_json::to_string(pcb_data)?;
    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title} - PasteBOM</title>
<style>
  body {{ font-family: sans-serif; margin: 0; padding: 20px; background: #1a1a2e; color: #eee; }}
  .container {{ max-width: 1200px; margin: 0 auto; }}
  h1 {{ color: #e94560; }}
  pre {{ background: #16213e; padding: 16px; border-radius: 8px; overflow: auto; max-height: 80vh; }}
</style>
</head>
<body>
<div class="container">
  <h1>{title}</h1>
  <p>Interactive viewer coming soon. Raw pcbdata JSON:</p>
  <pre id="pcbdata"></pre>
</div>
<script>
var pcbdata = {json};
document.getElementById('pcbdata').textContent = JSON.stringify(pcbdata, null, 2);
</script>
</body>
</html>"#,
        title = html_escape(title),
        json = json,
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
