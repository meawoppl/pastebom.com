//! Mapping URL suffixes onto files under a static asset directory.

use std::path::{Component, Path, PathBuf};

/// `dir/rel` when every component of `rel` is a plain name, so a request can
/// never leave `dir` (`..`, a leading `/` that would replace `dir` in
/// `Path::join`, Windows prefixes, or backslash separators).
pub fn resolve(dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\\') {
        return None;
    }
    let rel = Path::new(rel);
    rel.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| dir.join(rel))
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use std::path::Path;

    #[test]
    fn keeps_plain_relative_paths() {
        let dir = Path::new("/srv/assets");
        assert_eq!(
            resolve(dir, "index.html").unwrap(),
            Path::new("/srv/assets/index.html")
        );
        assert_eq!(
            resolve(dir, "a/b.wasm").unwrap(),
            Path::new("/srv/assets/a/b.wasm")
        );
    }

    #[test]
    fn rejects_escapes() {
        let dir = Path::new("/srv/assets");
        for rel in [
            "/etc/passwd",
            "../secret",
            "a/../../b",
            "./x",
            "a\\..\\b",
            "",
        ] {
            assert!(resolve(dir, rel).is_none(), "{rel}");
        }
    }
}
