//! Resolves JavaScript source descriptors into `(name, bytes)` pairs.
//!
//! Accepted forms, individually or in any iterable (Array, FileList, ...):
//! - a `File`
//! - `{ name, content }` where content is a string, ArrayBuffer, typed array, or Blob
//! - `{ name?, url }`, fetched with `fetch()`; the name defaults to the URL's last path segment

use js_sys::{ArrayBuffer, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, File, Response};

/// Split a `sources` argument into individual source descriptors.
pub fn to_list(sources: &JsValue) -> Result<Vec<JsValue>, JsValue> {
    if sources.is_instance_of::<File>() || !is_iterable(sources) {
        return Ok(vec![sources.clone()]);
    }
    let iter = js_sys::try_iter(sources)?.ok_or("sources must be iterable")?;
    iter.collect()
}

fn is_iterable(value: &JsValue) -> bool {
    value.is_object()
        && Reflect::get(value, &js_sys::Symbol::iterator())
            .map(|f| f.is_function())
            .unwrap_or(false)
}

/// Load a single source descriptor. Errors are human-readable strings.
pub async fn load(source: JsValue) -> Result<(String, Vec<u8>), String> {
    if let Some(file) = source.dyn_ref::<File>() {
        let bytes = blob_bytes(file)
            .await
            .map_err(|e| describe(&file.name(), &e))?;
        return Ok((file.name(), bytes));
    }
    if !source.is_object() {
        return Err("source must be a File or an object with `content` or `url`".into());
    }

    let name = get_string(&source, "name");
    let content = Reflect::get(&source, &"content".into()).unwrap_or(JsValue::UNDEFINED);
    let url = get_string(&source, "url");

    if !content.is_undefined() && !content.is_null() {
        let name = name.ok_or("source with `content` needs a `name`")?;
        let bytes = content_bytes(&content)
            .await
            .map_err(|e| describe(&name, &e))?;
        return Ok((name, bytes));
    }
    if let Some(url) = url {
        let name = name.unwrap_or_else(|| name_from_url(&url));
        let bytes = fetch_bytes(&url).await.map_err(|e| describe(&name, &e))?;
        return Ok((name, bytes));
    }
    Err(format!(
        "{}: source needs `content` or `url`",
        name.unwrap_or_else(|| "(unnamed)".into())
    ))
}

fn get_string(obj: &JsValue, key: &str) -> Option<String> {
    Reflect::get(obj, &key.into()).ok()?.as_string()
}

fn describe(name: &str, err: &JsValue) -> String {
    let msg = err
        .as_string()
        .or_else(|| {
            err.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .unwrap_or_else(|| format!("{err:?}"));
    format!("{name}: {msg}")
}

async fn content_bytes(content: &JsValue) -> Result<Vec<u8>, JsValue> {
    if let Some(s) = content.as_string() {
        return Ok(s.into_bytes());
    }
    if let Some(blob) = content.dyn_ref::<Blob>() {
        return blob_bytes(blob).await;
    }
    if content.is_instance_of::<ArrayBuffer>() || ArrayBuffer::is_view(content) {
        return Ok(Uint8Array::new(content).to_vec());
    }
    Err("unsupported `content` type (expected string, ArrayBuffer, typed array, or Blob)".into())
}

async fn blob_bytes(blob: &Blob) -> Result<Vec<u8>, JsValue> {
    let buf = JsFuture::from(blob.array_buffer()).await?;
    Ok(Uint8Array::new(&buf).to_vec())
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let resp: Response = JsFuture::from(window.fetch_with_str(url))
        .await?
        .dyn_into()?;
    if !resp.ok() {
        return Err(format!("HTTP {} fetching {url}", resp.status()).into());
    }
    let buf = JsFuture::from(resp.array_buffer()?).await?;
    Ok(Uint8Array::new(&buf).to_vec())
}

const NAME_QUERY_KEYS: [&str; 4] = ["path", "file", "filename", "name"];

/// Derive a filename from a URL. A `path`, `file`, `filename`, or `name` query
/// parameter wins (for file-serving APIs like `/api/file?path=gerbers/b.gtl`);
/// otherwise the last non-empty path segment is used.
pub fn name_from_url(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let (path, query) = without_fragment
        .split_once('?')
        .unwrap_or((without_fragment, ""));

    let from_query = query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        NAME_QUERY_KEYS
            .contains(&key)
            .then(|| percent_decode(&value.replace('+', " ")))
    });
    let source = from_query.as_deref().unwrap_or(path);
    source
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(url)
        .to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_urls() {
        assert_eq!(name_from_url("/fab/board-F_Cu.gbr"), "board-F_Cu.gbr");
        assert_eq!(name_from_url("https://x.test/a/b.GTL?rev=3#top"), "b.GTL");
        assert_eq!(name_from_url("/fab/dir/"), "dir");
    }

    #[test]
    fn names_from_file_api_query() {
        assert_eq!(
            name_from_url("/api/kicad/file?path=fab/board-B_Mask.gbr"),
            "board-B_Mask.gbr"
        );
        assert_eq!(
            name_from_url("/api/kicad/file?repo=x&path=fab%2Fgerbers%2Fboard.GTO"),
            "board.GTO"
        );
        assert_eq!(name_from_url("/get?filename=my+board.gbl"), "my board.gbl");
        assert_eq!(name_from_url("/files/b.drl?token=abc"), "b.drl");
    }
}
