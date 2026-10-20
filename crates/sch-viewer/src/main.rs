//! pastebom's schematic page at `/b/{id}/schematic`: loads the stored scene and
//! hosts `vector_view_yew::SchematicView`, keeping `?ref=` in the URL and
//! cross-probing to the board viewer at `/b/{id}#ref=`.
use std::rc::Rc;

use vector_view_yew::{
    schematic::DEFAULT_CSS,
    vector_view::{Scene, SCENE_VERSION},
    Reveal, SchematicView,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

fn main() {
    yew::Renderer::<App>::new().render();
}

/// Board id from `/b/{id}/schematic`.
fn board_id() -> Option<String> {
    let path = web_sys::window()?.location().pathname().ok()?;
    let mut parts = path.trim_matches('/').split('/');
    (parts.next()? == "b").then_some(())?;
    parts.next().map(str::to_owned)
}

/// `?ref=` from the page URL.
fn query_ref() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("ref").filter(|r| !r.is_empty())
}

/// Reflect the selected reference in the URL so the view can be shared.
fn set_query_ref(reference: Option<&str>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(path) = window.location().pathname() else {
        return;
    };
    let url = match reference {
        Some(r) => format!("{path}?ref={}", js_sys::encode_uri_component(r)),
        None => path,
    };
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
    }
}

fn board_href(id: &str, reference: Option<&str>) -> String {
    match reference {
        Some(r) => format!("/b/{id}#ref={}", js_sys::encode_uri_component(r)),
        None => format!("/b/{id}"),
    }
}

/// Board metadata relevant to the page chrome, from `/b/{id}/meta`.
#[derive(Clone, PartialEq)]
struct Meta {
    filename: String,
    has_pcb: bool,
}

fn parse_meta(v: &serde_json::Value) -> Meta {
    Meta {
        filename: v["filename"].as_str().unwrap_or_default().to_owned(),
        has_pcb: v["has_pcb"].as_bool().unwrap_or(true),
    }
}

async fn fetch_json(url: &str) -> Result<serde_json::Value, String> {
    let resp = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("{} ({})", resp.status_text(), resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

async fn fetch_scene(id: &str) -> Result<Scene, String> {
    let value = fetch_json(&format!("/b/{id}/schematic.json")).await?;
    let scene: Scene = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if scene.version != SCENE_VERSION {
        return Err(format!(
            "unsupported scene version {} (viewer expects {SCENE_VERSION})",
            scene.version
        ));
    }
    Ok(scene)
}

#[derive(Clone, PartialEq)]
enum Load {
    Loading,
    Ready(Rc<Scene>),
    Error(String),
}

#[function_component(App)]
fn app() -> Html {
    let id = use_memo((), |_| board_id().unwrap_or_default());
    let load = use_state(|| Load::Loading);
    let meta = use_state(|| None::<Meta>);
    let selected = use_state(|| None::<String>);
    let reveal = use_memo((), |_| {
        query_ref().map(|reference| Reveal { seq: 0, reference })
    });

    {
        let id = id.clone();
        let load = load.clone();
        let meta = meta.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(v) = fetch_json(&format!("/b/{id}/meta")).await {
                    meta.set(Some(parse_meta(&v)));
                }
                load.set(match fetch_scene(&id).await {
                    Ok(scene) => Load::Ready(Rc::new(scene)),
                    Err(e) => Load::Error(e),
                });
            });
        });
    }

    let scene = match &*load {
        Load::Loading => return html! { <div class="status">{"Loading schematic…"}</div> },
        Load::Error(e) => {
            return html! { <div class="status error">{format!("Could not load schematic: {e}")}</div> }
        }
        Load::Ready(scene) => scene.clone(),
    };

    let has_pcb = meta.as_ref().is_none_or(|m| m.has_pcb);
    let on_select_ref = {
        let selected = selected.clone();
        Callback::from(move |reference: Option<String>| {
            set_query_ref(reference.as_deref());
            selected.set(reference);
        })
    };
    let on_cross_probe = has_pcb.then(|| {
        let id = id.clone();
        Callback::from(move |reference: String| {
            if let Some(w) = web_sys::window() {
                let _ = w.location().set_href(&board_href(&id, Some(&reference)));
            }
        })
    });
    let pcb_link = has_pcb.then(|| {
        let href = board_href(&id, selected.as_deref());
        html! { <a class="button" {href}>{"PCB / BOM"}</a> }
    });
    let title = meta
        .as_ref()
        .map_or_else(|| "Schematic".to_owned(), |m| m.filename.clone());

    html! {
        <div class="app">
            <style>{DEFAULT_CSS}</style>
            <header>
                <a class="brand" href="/">{"pastebom"}</a>
                <span class="title">{title}</span>
                <span class="spacer" />
                {pcb_link}
            </header>
            <main>
                <SchematicView
                    {scene}
                    reveal={(*reveal).clone()}
                    {on_select_ref}
                    {on_cross_probe}
                    cross_probe_label="Show on PCB"
                />
            </main>
        </div>
    }
}
