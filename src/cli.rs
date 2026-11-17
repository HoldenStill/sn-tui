//! `clap` CLI definition for interactive TUI and headless automation subcommands.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::export::ExportFormat;

#[derive(Debug, Parser)]
#[command(
    name = "sn-tui",
    version,
    about = "Lean terminal-native Supernote .note/.mark/PDF viewer, exporter, and Cloud/Private Cloud sync manager"
)]
pub struct Cli {
    /// Optional path to config file (defaults to ~/.config/sn-tui/config.toml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Optional workspace directory to open in TUI mode (overrides config.workspace_dir)
    pub workspace: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Inspect a .note, .mark, or .pdf file and print structured JSON metadata
    Inspect {
        /// Input .note, .mark, or .pdf path
        file: PathBuf,
    },

    /// Convert a .note/.pdf file or an entire directory hierarchy
    Convert {
        /// Input file (.note / .pdf) or directory
        input: PathBuf,
        /// Output file or destination directory
        output: PathBuf,
        /// Target export format
        #[arg(short, long, value_enum, default_value_t = ExportFormat::PdfRaster)]
        format: ExportFormat,
        /// Max parallel worker threads (capped at 4 for low memory usage)
        #[arg(short = 'j', long, default_value_t = 4)]
        workers: usize,
    },

    /// Pull-sync remote Supernote Cloud / Private Cloud / WebDAV files to the local workspace
    Sync {
        /// Named profile from config.toml (uses default_profile if omitted)
        #[arg(short, long)]
        profile: Option<String>,
        /// Automatically export newly downloaded .note files after sync
        #[arg(long)]
        auto_export: bool,
        /// List changes without downloading or modifying local files
        #[arg(long)]
        dry_run: bool,
    },

    /// Explicitly upload a local PDF/template/file to a remote folder on Private Cloud / Cloud / WebDAV
    Upload {
        /// Local file to upload
        file: PathBuf,
        /// Destination remote directory (e.g. "/Document" or "/MyStyle")
        #[arg(short, long, default_value = "/Document")]
        dest: String,
        /// Named profile from config.toml (uses default_profile if omitted)
        #[arg(short, long)]
        profile: Option<String>,
    },

    /// Create a starter ~/.config/sn-tui/config.toml with 0600 permissions
    InitConfig {
        /// Overwrite existing config file if present
        #[arg(long)]
        force: bool,
    },
}
