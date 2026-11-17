//! Configuration loading (`~/.config/sn-tui/config.toml`), Unix `0600` credential checks,
//! and environment variable overrides (`SN_ACCOUNT`, `SN_PASSWORD`, `SN_TOKEN`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    PrivateCloud,
    SupernoteCloud,
    OpenPrivateCloud,
    Webdav,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProfile {
    pub kind: ProfileKind,
    /// Base URL (e.g. `https://sn.yourdomain.com`, `https://cloud.supernote.com`, or WebDAV root).
    pub base_url: Option<String>,
    pub account: Option<String>,
    pub password: Option<String>,
    /// Static Bearer token (`sn_...` for `allenporter/supernote` or WebDAV Bearer auth).
    pub token: Option<String>,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_true")]
    pub verify_tls: bool,
}

fn default_remote_root() -> String {
    "/".to_string()
}

fn default_true() -> bool {
    true
}

impl SyncProfile {
    pub fn resolved_base_url(&self) -> String {
        if let Some(url) = &self.base_url
            && !url.trim().is_empty()
        {
            return url.trim_end_matches('/').to_string();
        }
        match self.kind {
            ProfileKind::SupernoteCloud => "https://cloud.supernote.com".to_string(),
            _ => String::new(),
        }
    }

    /// Returns effective `(account, password, token)` after applying env overrides.
    pub fn resolved_credentials(&self) -> (Option<String>, Option<String>, Option<String>) {
        let account = std::env::var("SN_ACCOUNT")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.account.clone());
        let password = std::env::var("SN_PASSWORD")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.password.clone());
        let token = std::env::var("SN_TOKEN")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.token.clone());
        (account, password, token)
    }

    pub fn has_plaintext_secret(&self) -> bool {
        self.password.as_deref().is_some_and(|s| !s.is_empty())
            || self.token.as_deref().is_some_and(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_workspace_dir")]
    pub workspace_dir: PathBuf,
    #[serde(default = "default_export_dir")]
    pub export_dir: PathBuf,
    #[serde(default = "default_cache_max_mb")]
    pub cache_max_mb: u64,
    #[serde(default = "default_pdf_dpi")]
    pub pdf_render_dpi: u32,
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,
    pub auto_export_format: Option<String>,
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, SyncProfile>,
}

fn default_workspace_dir() -> PathBuf {
    BaseDirs::new()
        .map(|b| b.home_dir().join("Supernote"))
        .unwrap_or_else(|| PathBuf::from("./Supernote"))
}

fn default_export_dir() -> PathBuf {
    default_workspace_dir().join("Exports")
}

fn default_cache_max_mb() -> u64 {
    128
}

fn default_pdf_dpi() -> u32 {
    150
}

fn default_max_workers() -> usize {
    4
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            workspace_dir: default_workspace_dir(),
            export_dir: default_export_dir(),
            cache_max_mb: default_cache_max_mb(),
            pdf_render_dpi: default_pdf_dpi(),
            max_workers: default_max_workers(),
            auto_export_format: None,
            default_profile: None,
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub path: PathBuf,
    pub warnings: Vec<String>,
}

pub fn default_config_path() -> PathBuf {
    if let Some(proj) = ProjectDirs::from("", "", "sn-tui") {
        proj.config_dir().join("config.toml")
    } else if let Some(base) = BaseDirs::new() {
        base.home_dir().join(".config/sn-tui/config.toml")
    } else {
        PathBuf::from("sn-tui.toml")
    }
}

pub fn default_cache_dir() -> PathBuf {
    if let Some(proj) = ProjectDirs::from("", "", "sn-tui") {
        proj.cache_dir().to_path_buf()
    } else if let Some(base) = BaseDirs::new() {
        base.home_dir().join(".cache/sn-tui")
    } else {
        PathBuf::from(".cache/sn-tui")
    }
}

pub fn load_config(custom_path: Option<&Path>) -> Result<LoadedConfig> {
    let path = custom_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_path);
    let mut warnings = Vec::new();

    if !path.exists() {
        return Ok(LoadedConfig {
            config: AppConfig::default(),
            path,
            warnings,
        });
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let mut config: AppConfig = toml::from_str(&raw)
        .with_context(|| format!("failed to parse TOML config {}", path.display()))?;

    config.workspace_dir = expand_tilde(&config.workspace_dir);
    config.export_dir = expand_tilde(&config.export_dir);

    let has_secrets = config.profiles.values().any(SyncProfile::has_plaintext_secret);
    if has_secrets && let Some(warn) = check_unix_permissions(&path) {
        warnings.push(warn);
    }

    Ok(LoadedConfig {
        config,
        path,
        warnings,
    })
}

pub fn write_sample_config(path: &Path, overwrite: bool) -> Result<bool> {
    if path.exists() && !overwrite {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let sample = r#"# sn-tui configuration (~/.config/sn-tui/config.toml)
# Keep permissions locked to 0600 (`chmod 600 ~/.config/sn-tui/config.toml`) when storing secrets.
# Environment variables SN_ACCOUNT, SN_PASSWORD, and SN_TOKEN override profile credentials.

workspace_dir = "~/Supernote"
export_dir = "~/Supernote/Exports"
cache_max_mb = 128
pdf_render_dpi = 150
max_workers = 4
# auto_export_format = "pdf-raster"
# default_profile = "private"

# [profiles.private]
# kind = "private_cloud"
# base_url = "https://supernote.local"
# account = "you@example.com"
# password = "your-password"
# remote_root = "/"
# verify_tls = true

# [profiles.cloud]
# kind = "supernote_cloud"
# base_url = "https://cloud.supernote.com"
# account = "you@example.com"
# password = "your-password"
# remote_root = "/"

# [profiles.open_cloud]
# kind = "open_private_cloud"
# base_url = "http://127.0.0.1:8000"
# token = "sn_your_bearer_token"
# remote_root = "/"

# [profiles.webdav]
# kind = "webdav"
# base_url = "https://dav.example.com/Supernote"
# account = "username"
# password = "password"
# remote_root = "/"
"#;

    fs::write(path, sample)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(true)
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(base) = BaseDirs::new()
    {
        return base.home_dir().join(rest);
    }
    path.to_path_buf()
}

#[cfg(unix)]
fn check_unix_permissions(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path).ok()?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        Some(format!(
            "Security warning: {} has permissions {:04o} with plaintext credentials; run `chmod 600 {}`",
            path.display(),
            mode,
            path.display()
        ))
    } else {
        None
    }
}

#[cfg(not(unix))]
fn check_unix_permissions(_path: &Path) -> Option<String> {
    None
}
