use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Serialize;

use sn_tui::cli::{Cli, Commands};
use sn_tui::config::{default_config_path, load_config, write_sample_config};
use sn_tui::export::export_path;
use sn_tui::note::{Notebook, extract_notebook_text, parse_notebook_file};
use sn_tui::pdf::{PdfInfo, inspect_pdf};

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum InspectOutput {
    Supernote {
        path: String,
        notebook: Notebook,
        recognized_pages: Vec<(u32, String)>,
    },
    Pdf {
        path: String,
        info: PdfInfo,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let loaded = load_config(cli.config.as_deref())?;
    for warn in &loaded.warnings {
        eprintln!("warning: {warn}");
    }

    match cli.command {
        Some(Commands::InitConfig { force }) => {
            let target = cli.config.unwrap_or_else(default_config_path);
            if write_sample_config(&target, force)? {
                println!("Created starter config at {} (mode 0600)", target.display());
            } else {
                println!(
                    "Config already exists at {} (use --force to overwrite)",
                    target.display()
                );
            }
        }
        Some(Commands::Inspect { file }) => {
            run_inspect(&file)?;
        }
        Some(Commands::Convert {
            input,
            output,
            format,
            workers,
        }) => {
            let summary = export_path(&input, &output, format, workers)?;
            println!(
                "Exported {} file(s) ({} skipped) using {}",
                summary.files_exported,
                summary.skipped,
                format.display_name()
            );
            for out in &summary.outputs {
                println!("  -> {}", out.display());
            }
        }
        Some(Commands::Sync { .. }) => {
            bail!("sync command will be wired in Milestone 5");
        }
        Some(Commands::Upload { .. }) => {
            bail!("upload command will be wired in Milestone 5");
        }
        None => {
            let ws = cli.workspace.unwrap_or(loaded.config.workspace_dir);
            println!(
                "TUI workspace {} ready (launching interactive TUI will be wired in Milestone 4)",
                ws.display()
            );
        }
    }

    Ok(())
}

fn run_inspect(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("file not found: {}", path.display());
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let output = match ext.as_str() {
        "note" | "mark" => {
            let notebook = parse_notebook_file(path)?;
            let recognized_pages = extract_notebook_text(path).unwrap_or_default();
            InspectOutput::Supernote {
                path: path.display().to_string(),
                notebook,
                recognized_pages,
            }
        }
        "pdf" => {
            let info = inspect_pdf(path)?;
            InspectOutput::Pdf {
                path: path.display().to_string(),
                info,
            }
        }
        _ => bail!("unsupported file type for inspect: {}", path.display()),
    };

    let json = serde_json::to_string_pretty(&output).context("failed to serialize inspect JSON")?;
    println!("{json}");
    Ok(())
}
