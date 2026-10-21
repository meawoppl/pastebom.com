//! Optional built-in layer panel. Hosts that draw their own chrome leave it off
//! and drive the viewer through the JS API instead.
//!
//! Elements carry `gv-*` classes so hosts can restyle them. Interaction is
//! delegated: checkboxes have `data-gv-layer="<index>"`, buttons `data-gv-action`.

pub const PANEL_STYLE: &str = "position:absolute;top:8px;left:8px;max-height:calc(100% - 16px);\
overflow:auto;padding:6px 8px;background:rgba(18,22,28,0.85);color:#d7dae0;\
font:12px/1.4 system-ui,sans-serif;border-radius:6px;user-select:none;";

pub struct PanelRow<'a> {
    pub label: &'a str,
    pub name: &'a str,
    pub color: &'a str,
    pub visible: bool,
}

pub fn panel_html(rows: &[PanelRow], from_bottom: bool) -> String {
    let side_button = if from_bottom {
        "Top view"
    } else {
        "Bottom view"
    };
    let mut html = format!(
        "<div class=\"gv-toolbar\" style=\"display:flex;gap:4px;margin-bottom:4px\">\
<button type=\"button\" class=\"gv-button\" data-gv-action=\"side\">{side_button}</button>\
<button type=\"button\" class=\"gv-button\" data-gv-action=\"fit\">Fit</button></div>"
    );
    for (i, row) in rows.iter().enumerate() {
        html.push_str(&format!(
            "<label class=\"gv-layer\" title=\"{name}\" style=\"display:flex;align-items:center;gap:6px;white-space:nowrap\">\
<input type=\"checkbox\" data-gv-layer=\"{i}\"{checked}>\
<span class=\"gv-swatch\" style=\"display:inline-block;width:10px;height:10px;border-radius:2px;background:{color}\"></span>\
{label}</label>",
            name = escape(row.name),
            checked = if row.visible { " checked" } else { "" },
            color = escape(row.color),
            label = escape(row.label),
        ));
    }
    html
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_escaped_and_indexed() {
        let rows = [
            PanelRow {
                label: "Top copper",
                name: "a<b>.gtl",
                color: "red\" onclick=\"x",
                visible: true,
            },
            PanelRow {
                label: "Drills",
                name: "d.drl",
                color: "#000",
                visible: false,
            },
        ];
        let html = panel_html(&rows, false);
        assert!(html.contains("data-gv-layer=\"0\" checked"));
        assert!(html.contains("data-gv-layer=\"1\">"));
        assert!(html.contains("a&lt;b&gt;.gtl"));
        assert!(!html.contains("\" onclick"));
        assert!(html.contains("Bottom view"));
    }
}
