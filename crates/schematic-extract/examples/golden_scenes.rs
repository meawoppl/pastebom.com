//! Render a KiCadmium capture corpus for semantic scene comparison.
use std::{collections::HashMap, path::Path};

fn collect(root: &Path, dir: &Path, files: &mut HashMap<String, Vec<u8>>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(root, &path, files)?;
        } else if entry.file_type()?.is_file() && path.extension().is_some_and(|e| e == "kicad_sch")
        {
            files.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                std::fs::read(path)?,
            );
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let root = Path::new(args.get(1).ok_or("usage: golden_scenes CORPUS OUTPUT")?);
    let output = Path::new(args.get(2).ok_or("missing output directory")?);
    std::fs::create_dir_all(output)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("MANIFEST.json"))?)?;
    let options = schematic_extract::ExtractOptions {
        frame_label: Some("kicadmium".into()),
        ..Default::default()
    };
    for (name, project) in manifest["projects"].as_object().ok_or("missing projects")? {
        let src = root.join(name).join("src");
        let mut files = HashMap::new();
        collect(&src, &src, &mut files)?;
        let sch = project["source"]["kicad"]["schematic"]
            .as_str()
            .ok_or("missing root schematic")?;
        let scene = schematic_extract::parse_project_with(sch, &files, &options)?;
        std::fs::write(
            output.join(format!("{name}.json")),
            serde_json::to_vec(&scene)?,
        )?;
        eprintln!("{name}: {} items", scene.items.len());
        let library_output = output.join(name);
        std::fs::create_dir_all(&library_output)?;
        for entry in std::fs::read_dir(root.join(name).join("golden"))? {
            let path = entry?.path();
            let Some(file) = path.file_name().and_then(|p| p.to_str()) else {
                continue;
            };
            let Some(stem) = file.strip_suffix(".source.kicad_sch") else {
                continue;
            };
            let tree =
                schematic_extract::kicad_schema::sexp::parse(&std::fs::read_to_string(&path)?)?;
            let symbol = tree
                .get("lib_symbols")
                .and_then(|n| n.get("symbol"))
                .ok_or("missing library symbol")?;
            let mut model = schematic_extract::LibrarySymbol::from_sexp(symbol);
            // The source endpoint wraps a library symbol with its qualified id;
            // the preview model uses the short symbol name.
            if let Some((_, short)) = model.name.split_once(':') {
                model.name = short.to_owned();
            }
            let preview = schematic_extract::symbol_scene(&model, &options.palette);
            std::fs::write(
                library_output.join(format!("{stem}.json")),
                serde_json::to_vec(&preview)?,
            )?;
        }
    }
    Ok(())
}
