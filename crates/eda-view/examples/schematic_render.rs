//! Render a native project: cargo run -p schematic-extract --example render -- root.kicad_sch output.svg
use std::collections::HashMap;
use std::path::Path;

fn collect(root: &Path, dir: &Path, files: &mut HashMap<String, Vec<u8>>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect(root, &path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "kicad_sch") {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(name, std::fs::read(path)?);
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: render ROOT.kicad_sch OUTPUT.svg".into());
    }
    let input = Path::new(&args[1]).canonicalize()?;
    let mut files = HashMap::new();
    collect(input.parent().unwrap(), input.parent().unwrap(), &mut files)?;
    let scene =
        eda_view::schematic::parse_project(input.file_name().unwrap().to_str().unwrap(), &files)?;
    let item_ids = scene
        .items
        .iter()
        .filter(|item| item.props.iter().any(|p| p.key == "page" && p.value == "/"))
        .map(|i| i.id)
        .collect();
    let svg = scene.to_svg(&eda_view::vector::svg::SvgOptions {
        item_ids: Some(item_ids),
        background: Some([0x1a, 0x1b, 0x26, 255]),
        ..Default::default()
    });
    std::fs::write(&args[2], svg)?;
    eprintln!(
        "{} items, {} groups, {} layers",
        scene.items.len(),
        scene.groups.len(),
        scene.layers.len()
    );
    for warning in scene.meta.iter().filter(|p| p.key == "warning") {
        eprintln!("{}", warning.value);
    }
    Ok(())
}
