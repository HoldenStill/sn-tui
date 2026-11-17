//! Pull-only mirror sync orchestrator, explicit upload dispatcher, and incremental sync manifest.

pub mod sncloud;
pub mod watcher;
pub mod webdav;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, ProfileKind, SyncProfile};
use sncloud::SnCloudClient;
use webdav::WebDavClient;

#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub remote_path: String,
    pub id: String,
    pub size: u64,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub downloaded_paths: Vec<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncManifest {
    /// Maps `<profile_key>::<remote_path>` -> `(size, fingerprint)`.
    entries: HashMap<String, (u64, String)>,
}

impl SyncManifest {
    fn load(cache_root: &Path) -> Self {
        let p = cache_root.join("sync_state.json");
        fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, cache_root: &Path) -> Result<()> {
        fs::create_dir_all(cache_root)?;
        let p = cache_root.join("sync_state.json");
        let tmp = cache_root.join("sync_state.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, &p)?;
        Ok(())
    }
}

pub fn resolve_profile<'a>(
    config: &'a AppConfig,
    explicit: Option<&str>,
) -> Result<(String, &'a SyncProfile)> {
    let name = explicit
        .map(String::from)
        .or_else(|| config.default_profile.clone())
        .or_else(|| config.profiles.keys().next().cloned())
        .context(
            "no sync profile configured; run `sn-tui init-config` and configure a profile in ~/.config/sn-tui/config.toml",
        )?;

    let profile = config
        .profiles
        .get(&name)
        .with_context(|| format!("sync profile `{name}` not found in config.toml"))?;
    Ok((name, profile))
}

/// Safe pull-only incremental mirror from Cloud / Private Cloud / WebDAV into `local_root`.
/// Never deletes or mutates remote files.
pub fn pull_sync<F>(
    profile_name: &str,
    profile: &SyncProfile,
    local_root: &Path,
    cache_root: &Path,
    dry_run: bool,
    mut on_progress: F,
) -> Result<SyncReport>
where
    F: FnMut(&str),
{
    fs::create_dir_all(local_root)?;
    let mut manifest = SyncManifest::load(cache_root);
    let mut report = SyncReport::default();

    on_progress(&format!(
        "Connecting to profile `{profile_name}` ({:?})...",
        profile.kind
    ));

    match profile.kind {
        ProfileKind::PrivateCloud | ProfileKind::SupernoteCloud | ProfileKind::OpenPrivateCloud => {
            let client = SnCloudClient::connect(profile)?;
            let remote_files = client.list_recursive(&profile.remote_root)?;
            on_progress(&format!(
                "Found {} remote file(s); checking incremental diff...",
                remote_files.len()
            ));

            for entry in remote_files {
                let local_dest = safe_join(local_root, &entry.remote_path)?;
                let state_key = format!("{profile_name}::{}", entry.remote_path);
                if is_up_to_date(&local_dest, &entry, manifest.entries.get(&state_key)) {
                    report.skipped += 1;
                    continue;
                }
                if dry_run {
                    on_progress(&format!("[dry-run] would pull {}", entry.remote_path));
                    report.downloaded += 1;
                    continue;
                }

                on_progress(&format!("Pulling {}...", entry.remote_path));
                match client.download_file(&entry) {
                    Ok(bytes) => {
                        write_atomic(&local_dest, &bytes)?;
                        manifest
                            .entries
                            .insert(state_key, (entry.size, entry.fingerprint.clone()));
                        report.downloaded += 1;
                        if report.downloaded.is_multiple_of(10) {
                            let _ = manifest.save(cache_root);
                        }
                        report.downloaded_paths.push(local_dest);
                    }
                    Err(e) => {
                        on_progress(&format!("ERR pulling {}: {e:#}", entry.remote_path));
                        report.failed += 1;
                    }
                }
            }
        }
        ProfileKind::Webdav => {
            let client = WebDavClient::connect(profile)?;
            let remote_files = client.list_recursive(&profile.remote_root)?;
            on_progress(&format!(
                "Found {} WebDAV file(s); checking incremental diff...",
                remote_files.len()
            ));

            for entry in remote_files {
                let local_dest = safe_join(local_root, &entry.remote_path)?;
                let state_key = format!("{profile_name}::{}", entry.remote_path);
                if is_up_to_date(&local_dest, &entry, manifest.entries.get(&state_key)) {
                    report.skipped += 1;
                    continue;
                }
                if dry_run {
                    on_progress(&format!("[dry-run] would pull {}", entry.remote_path));
                    report.downloaded += 1;
                    continue;
                }

                on_progress(&format!("Pulling {}...", entry.remote_path));
                match client.download_file(&entry) {
                    Ok(bytes) => {
                        write_atomic(&local_dest, &bytes)?;
                        manifest
                            .entries
                            .insert(state_key, (entry.size, entry.fingerprint.clone()));
                        report.downloaded += 1;
                        if report.downloaded.is_multiple_of(10) {
                            let _ = manifest.save(cache_root);
                        }
                        report.downloaded_paths.push(local_dest);
                    }
                    Err(e) => {
                        on_progress(&format!("ERR pulling {}: {e:#}", entry.remote_path));
                        report.failed += 1;
                    }
                }
            }
        }
    }

    if !dry_run {
        let _ = manifest.save(cache_root);
    }

    Ok(report)
}

/// Explicitly uploads a local file to `remote_dest` on the configured profile.
pub fn upload_file(profile: &SyncProfile, local_file: &Path, remote_dest: &str) -> Result<()> {
    if !local_file.is_file() {
        bail!("local file does not exist: {}", local_file.display());
    }
    match profile.kind {
        ProfileKind::PrivateCloud | ProfileKind::SupernoteCloud | ProfileKind::OpenPrivateCloud => {
            let client = SnCloudClient::connect(profile)?;
            client.upload_file(local_file, remote_dest)
        }
        ProfileKind::Webdav => {
            let client = WebDavClient::connect(profile)?;
            client.upload_file(local_file, remote_dest)
        }
    }
}

fn is_up_to_date(
    local_path: &Path,
    remote: &RemoteEntry,
    cached: Option<&(u64, String)>,
) -> bool {
    let Ok(meta) = fs::metadata(local_path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if let Some((prev_size, prev_fp)) = cached {
        let size_ok = remote.size == 0 || (meta.len() == remote.size && *prev_size == remote.size);
        let fp_ok = !remote.fingerprint.is_empty() && *prev_fp == remote.fingerprint;
        if size_ok && fp_ok {
            return true;
        }
    }
    false
}

/// Prevents path traversal (`..`) from untrusted remote filenames.
fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            std::path::Component::Normal(seg) => path.push(seg),
            std::path::Component::CurDir => {}
            _ => bail!("unsafe remote path rejected: {rel}"),
        }
    }
    Ok(path)
}

fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("sn_tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dest)?;
    Ok(())
}
