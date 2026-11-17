use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use serde::Serialize;

use sn_tui::cli::{Cli, Commands};
use sn_tui::config::{default_cache_dir, default_config_path, load_config, write_sample_config};
use sn_tui::export::{ExportFormat, export_path};
use sn_tui::note::{Notebook, extract_notebook_text, parse_notebook_file};
use sn_tui::pdf::{PdfInfo, inspect_pdf};
use sn_tui::sync::{pull_sync, resolve_profile, upload_file};

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
        Some(Commands::Sync {
            profile,
            auto_export,
            dry_run,
        }) => {
            let (name, prof) = resolve_profile(&loaded.config, profile.as_deref())?;
            let cache_dir = default_cache_dir();
            let report = pull_sync(
                &name,
                prof,
                &loaded.config.workspace_dir,
                &cache_dir,
                dry_run,
                |msg| println!("{msg}"),
            )?;
            println!(
                "Sync `{name}` summary: {} pulled, {} skipped (up-to-date), {} failed",
                report.downloaded, report.skipped, report.failed
            );

            if auto_export && !dry_run && !report.downloaded_paths.is_empty() {
                let fmt = loaded
                    .config
                    .auto_export_format
                    .as_deref()
                    .and_then(|s| ExportFormat::from_str(s, true).ok())
                    .unwrap_or(ExportFormat::PdfRaster);
                for path in &report.downloaded_paths {
                    if path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("note"))
                    {
                        let rel = path
                            .strip_prefix(&loaded.config.workspace_dir)
                            .unwrap_or(path);
                        let out = loaded
                            .config
                            .export_dir
                            .join(rel)
                            .with_extension(fmt.default_extension());
                        if let Err(e) = export_path(path, &out, fmt, loaded.config.max_workers) {
                            eprintln!("auto-export failed for {}: {e:#}", path.display());
                        } else {
                            println!("  auto-exported -> {}", out.display());
                        }
                    }
                }
            }
        }
        Some(Commands::Upload {
            file,
            dest,
            profile,
        }) => {
            let (name, prof) = resolve_profile(&loaded.config, profile.as_deref())?;
            upload_file(prof, &file, &dest)?;
            println!(
                "Uploaded {} to `{dest}` via profile `{name}`",
                file.display()
            );
        }
        None => {
            let mut cfg = loaded.config;
            if let Some(ws) = cli.workspace {
                cfg.workspace_dir = ws;
            }
            sn_tui::tui::run_tui(cfg, loaded.warnings)?;
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
