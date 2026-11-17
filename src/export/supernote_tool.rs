//! Optional subprocess bridge to `supernote-tool` (or `uvx --from supernotelib supernote-tool`)
//! strictly for Vector PDF (`--pdf-type vector`) and SVG (`-t svg`) exports.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupernoteToolBackend {
    DirectBinary,
    Uvx,
    Unavailable,
}

impl SupernoteToolBackend {
    pub fn detect() -> Self {
        if command_exists("supernote-tool", &["--version"]) {
            Self::DirectBinary
        } else if command_exists("uvx", &["--version"]) {
            Self::Uvx
        } else {
            Self::Unavailable
        }
    }

    pub fn is_available(self) -> bool {
        !matches!(self, Self::Unavailable)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DirectBinary => "supernote-tool",
            Self::Uvx => "uvx (supernotelib)",
            Self::Unavailable => "not installed",
        }
    }

    fn build_command(self) -> Result<Command> {
        match self {
            Self::DirectBinary => Ok(Command::new("supernote-tool")),
            Self::Uvx => {
                let mut cmd = Command::new("uvx");
                cmd.args(["--from", "supernotelib", "supernote-tool"]);
                Ok(cmd)
            }
            Self::Unavailable => bail!(
                "Vector PDF / SVG export requires `supernote-tool` or `uvx` on PATH. \
                 Install via `pipx install supernotelib` or `uv tool install supernotelib`, \
                 or use native `pdf-raster`, `png`, `txt`, or `md` formats instead."
            ),
        }
    }

    pub fn export_vector_pdf(self, input: &Path, output: &Path) -> Result<()> {
        if let Some(parent) = output.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut cmd = self.build_command()?;
        let out = cmd
            .args(["convert", "-t", "pdf", "--pdf-type", "vector", "-a"])
            .arg(input)
            .arg(output)
            .output()
            .context("failed to spawn supernote-tool for vector PDF export")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("supernote-tool vector PDF failed: {}", stderr.trim());
        }
        Ok(())
    }

    pub fn export_svg(self, input: &Path, output: &Path) -> Result<()> {
        if let Some(parent) = output.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut cmd = self.build_command()?;
        let out = cmd
            .args(["convert", "-t", "svg", "-a"])
            .arg(input)
            .arg(output)
            .output()
            .context("failed to spawn supernote-tool for SVG export")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("supernote-tool SVG failed: {}", stderr.trim());
        }
        Ok(())
    }
}

fn command_exists(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}
