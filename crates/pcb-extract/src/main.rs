use clap::Parser;
use pcb_extract::{extract, ExtractOptions};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pcb-extract", about = "Extract PCB data to JSON")]
struct Cli {
    /// Input PCB file (.kicad_pcb, .json, .brd, .pcbdoc)
    input: PathBuf,

    /// Output JSON file (stdout if not specified)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Include tracks in output
    #[arg(long)]
    tracks: bool,

    /// Include nets in output
    #[arg(long)]
    nets: bool,
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let opts = ExtractOptions {
        include_tracks: cli.tracks,
        include_nets: cli.nets,
    };

    match extract(&cli.input, &opts) {
        Ok(pcb_data) => {
            let json = serde_json::to_string_pretty(&pcb_data).expect("JSON serialization failed");
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
