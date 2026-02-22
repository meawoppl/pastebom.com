use clap::Parser;
use pcb_extract::{extract, extract_bytes, ExtractOptions, PcbFormat};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pcb-extract", about = "Extract PCB data to JSON")]
struct Cli {
    /// Input PCB file (.kicad_pcb, .json, .brd, .pcbdoc, .zip)
    input: PathBuf,

    /// Output JSON file (stdout if not specified)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Override auto-detected format (kicad, easyeda, eagle, altium, gerber)
    #[arg(short, long)]
    format: Option<String>,

    /// Pretty-print JSON output
    #[arg(long)]
    pretty: bool,

    /// Include tracks in output
    #[arg(long)]
    tracks: bool,

    /// Include nets in output
    #[arg(long)]
    nets: bool,
}

fn parse_format(s: &str) -> Result<PcbFormat, String> {
    match s.to_lowercase().as_str() {
        "kicad" => Ok(PcbFormat::KiCad),
        "easyeda" => Ok(PcbFormat::EasyEda),
        "eagle" => Ok(PcbFormat::Eagle),
        "altium" => Ok(PcbFormat::Altium),
        "gerber" => Ok(PcbFormat::Gerber),
        _ => Err(format!(
            "Unknown format: {s}. Use: kicad, easyeda, eagle, altium, gerber"
        )),
    }
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let opts = ExtractOptions {
        include_tracks: cli.tracks,
        include_nets: cli.nets,
    };

    let result = if let Some(fmt_str) = &cli.format {
        let format = match parse_format(fmt_str) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        };
        let data = match std::fs::read(&cli.input) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error reading file: {e}");
                std::process::exit(1);
            }
        };
        extract_bytes(&data, format, &opts)
    } else {
        extract(&cli.input, &opts)
    };

    match result {
        Ok(pcb_data) => {
            let json = if cli.pretty {
                serde_json::to_string_pretty(&pcb_data)
            } else {
                serde_json::to_string(&pcb_data)
            }
            .expect("JSON serialization failed");

            if let Some(output_path) = cli.output {
                std::fs::write(&output_path, &json).expect("Failed to write output file");
                eprintln!("Written to {}", output_path.display());
            } else {
                println!("{json}");
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
