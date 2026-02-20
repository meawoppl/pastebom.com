use pcb_extract::types::PcbData;

/// Generate a self-contained HTML page that renders the interactive BOM viewer.
///
/// Embeds the pcbdata JSON and renders a BOM table with component grouping.
/// The pcbdata is also available as a JS variable for future interactive viewer.
pub fn generate_html(pcb_data: &PcbData, title: &str) -> Result<String, serde_json::Error> {
    let json = serde_json::to_string(pcb_data)?;
    let escaped_title = html_escape(title);
    let bom_table = build_bom_table(pcb_data);
    let stats = build_stats(pcb_data);

    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title} - PasteBOM</title>
<style>
:root {{
  --bg: #1a1a2e;
  --surface: #16213e;
  --accent: #e94560;
  --text: #eee;
  --muted: #aaa;
  --border: #2a2a4a;
}}
body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; margin: 0; padding: 20px; background: var(--bg); color: var(--text); }}
.container {{ max-width: 1200px; margin: 0 auto; }}
h1 {{ color: var(--accent); margin-bottom: 4px; }}
.stats {{ color: var(--muted); margin-bottom: 24px; font-size: 14px; }}
.tabs {{ display: flex; gap: 0; margin-bottom: 0; }}
.tab {{ padding: 10px 20px; background: var(--surface); border: 1px solid var(--border); border-bottom: none; cursor: pointer; color: var(--muted); border-radius: 8px 8px 0 0; }}
.tab.active {{ color: var(--text); background: var(--border); }}
.tab-content {{ display: none; }}
.tab-content.active {{ display: block; }}
table {{ width: 100%; border-collapse: collapse; background: var(--surface); border-radius: 0 8px 8px 8px; overflow: hidden; }}
th {{ text-align: left; padding: 12px 16px; background: var(--border); font-weight: 600; font-size: 13px; text-transform: uppercase; letter-spacing: 0.5px; }}
td {{ padding: 10px 16px; border-bottom: 1px solid var(--border); font-size: 14px; }}
tr:hover td {{ background: rgba(233, 69, 96, 0.05); }}
.ref-list {{ font-family: 'SF Mono', Monaco, monospace; font-size: 13px; }}
.count {{ font-weight: 600; color: var(--accent); text-align: center; }}
.search-box {{ width: 100%; padding: 10px 16px; background: var(--surface); border: 1px solid var(--border); border-radius: 8px; color: var(--text); font-size: 14px; margin-bottom: 16px; box-sizing: border-box; outline: none; }}
.search-box:focus {{ border-color: var(--accent); }}
.json-toggle {{ margin-top: 24px; }}
.json-toggle summary {{ cursor: pointer; color: var(--muted); font-size: 13px; }}
pre {{ background: var(--surface); padding: 16px; border-radius: 8px; overflow: auto; max-height: 60vh; font-size: 12px; }}
</style>
</head>
<body>
<div class="container">
  <h1>{title}</h1>
  <div class="stats">{stats}</div>
  <input type="text" class="search-box" id="search" placeholder="Search components..." oninput="filterBom()">

  <div class="tabs">
    <div class="tab active" onclick="switchTab('both')">All</div>
    <div class="tab" onclick="switchTab('front')">Front</div>
    <div class="tab" onclick="switchTab('back')">Back</div>
  </div>

  {bom_table}

  <details class="json-toggle">
    <summary>Raw pcbdata JSON</summary>
    <pre id="pcbdata"></pre>
  </details>
</div>
<script>
var pcbdata = {json};
document.getElementById('pcbdata').textContent = JSON.stringify(pcbdata, null, 2);

function switchTab(tab) {{
  document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
  document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
  event.target.classList.add('active');
  document.getElementById('tab-' + tab).classList.add('active');
}}

function filterBom() {{
  var query = document.getElementById('search').value.toLowerCase();
  document.querySelectorAll('table tbody tr').forEach(function(row) {{
    var text = row.textContent.toLowerCase();
    row.style.display = text.includes(query) ? '' : 'none';
  }});
}}
</script>
</body>
</html>"#,
        title = escaped_title,
        stats = stats,
        bom_table = bom_table,
        json = json,
    ))
}

fn build_bom_table(pcb_data: &PcbData) -> String {
    let bom = match &pcb_data.bom {
        Some(b) => b,
        None => return "<p>No BOM data available.</p>".to_string(),
    };

    let mut html = String::new();

    for (tab_id, rows) in [
        ("both", &bom.both),
        ("front", &bom.front),
        ("back", &bom.back),
    ] {
        let active = if tab_id == "both" { " active" } else { "" };
        html.push_str(&format!(
            r#"<div id="tab-{tab_id}" class="tab-content{active}"><table><thead><tr><th>#</th><th>Qty</th><th>References</th><th>Value</th><th>Footprint</th></tr></thead><tbody>"#
        ));

        for (i, group) in rows.iter().enumerate() {
            let refs: Vec<&str> = group.iter().map(|(r, _)| r.as_str()).collect();
            let ref_list = refs.join(", ");
            let qty = group.len();

            // Get value and footprint from fields map
            let (value, footprint) = if let Some(first) = group.first() {
                let idx = first.1.to_string();
                if let Some(fields) = bom.fields.0.get(&idx) {
                    (
                        fields.first().map(|s| s.as_str()).unwrap_or(""),
                        fields.get(1).map(|s| s.as_str()).unwrap_or(""),
                    )
                } else {
                    ("", "")
                }
            } else {
                ("", "")
            };

            html.push_str(&format!(
                "<tr><td>{row}</td><td class=\"count\">{qty}</td><td class=\"ref-list\">{refs}</td><td>{value}</td><td>{footprint}</td></tr>",
                row = i + 1,
                qty = qty,
                refs = html_escape(&ref_list),
                value = html_escape(value),
                footprint = html_escape(footprint),
            ));
        }

        html.push_str("</tbody></table></div>");
    }

    html
}

fn build_stats(pcb_data: &PcbData) -> String {
    let total_fps = pcb_data.footprints.len();
    let (front, back) = if let Some(bom) = &pcb_data.bom {
        let front_count: usize = bom.front.iter().map(|g| g.len()).sum();
        let back_count: usize = bom.back.iter().map(|g| g.len()).sum();
        (front_count, back_count)
    } else {
        (0, 0)
    };
    let unique_groups = pcb_data.bom.as_ref().map(|b| b.both.len()).unwrap_or(0);

    format!("{total_fps} components ({front} front, {back} back) | {unique_groups} unique groups")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
