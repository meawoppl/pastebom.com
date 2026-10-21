//! Deterministic root-sheet discovery for in-memory project bundles.

use std::collections::{HashMap, HashSet};

use crate::kicad::sexp;

/// Choose a root without accessing the filesystem or loading symbol libraries.
/// Malformed files stay eligible so the project parser can report their errors.
pub(crate) fn find_root(files: &HashMap<String, Vec<u8>>) -> Option<String> {
    let mut candidates: Vec<_> = files
        .keys()
        .filter_map(|name| {
            let normalized = resolve("", name)?;
            if is_metadata(name) || !has_extension(&normalized, "kicad_sch") {
                return None;
            }
            Some((name, normalized))
        })
        .collect();
    candidates.sort_by(|a, b| {
        a.1.split('/')
            .count()
            .cmp(&b.1.split('/').count())
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.0.cmp(b.0))
    });

    let companions: HashSet<_> = files
        .keys()
        .filter_map(|name| {
            if is_metadata(name) {
                return None;
            }
            let normalized = resolve("", name)?;
            if has_extension(&normalized, "kicad_pro") || has_extension(&normalized, "kicad_pcb") {
                Some(stem(&normalized).to_owned())
            } else {
                None
            }
        })
        .collect();
    if let Some((name, _)) = candidates
        .iter()
        .find(|(_, path)| companions.contains(stem(path)))
    {
        return Some((*name).clone());
    }

    let mut children = HashSet::new();
    for (name, normalized) in &candidates {
        let Ok(text) = std::str::from_utf8(&files[*name]) else {
            continue;
        };
        // The shared parser bounds nesting to 128 levels and rejects non-finite
        // numbers. Only direct sheet declarations determine the dependency graph.
        let Ok(tree) = sexp::parse(text) else {
            continue;
        };
        if !tree.has_tag("kicad_sch") {
            continue;
        }
        let parent = normalized.rsplit_once('/').map_or("", |(dir, _)| dir);
        for sheet in tree.children_named("sheet") {
            let Some(file) = sheet.property("Sheetfile") else {
                continue;
            };
            if is_metadata(file) {
                continue;
            }
            if let Some(child) = resolve(parent, file) {
                // Self references do not hide an otherwise eligible root;
                // recursive hierarchy errors belong to the project parser.
                if child != *normalized {
                    children.insert(child);
                }
            }
        }
    }
    candidates
        .iter()
        .find(|(_, path)| !children.contains(path))
        .or_else(|| candidates.first())
        .map(|(name, _)| (*name).clone())
}

fn is_metadata(path: &str) -> bool {
    path.split('/')
        .any(|part| part == "__MACOSX" || part.starts_with("._"))
}

fn has_extension(path: &str, extension: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case(extension))
}

fn stem(path: &str) -> &str {
    path.rsplit_once('.').map_or(path, |(stem, _)| stem)
}

/// Resolve a relative member name lexically, rejecting paths outside the bundle.
fn resolve(parent: &str, file: &str) -> Option<String> {
    if file.starts_with('/') || file.contains(['\\', ':']) {
        return None;
    }
    let mut parts: Vec<_> = parent.split('/').filter(|part| !part.is_empty()).collect();
    for part in file.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}
