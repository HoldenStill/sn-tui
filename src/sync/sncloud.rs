//! Native blocking `ureq` client for Official Ratta Supernote Private Cloud (Docker),
//! Free Supernote Cloud (`https://cloud.supernote.com`), and `allenporter/supernote`
//! Open Private Cloud (`Authorization: Bearer sn_...`).

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use md5::{Digest as Md5Digest, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use ureq::Agent;
use ureq::tls::TlsConfig;

use crate::config::{ProfileKind, SyncProfile};
use crate::sync::RemoteEntry;

const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) sn-tui/0.1";

pub struct SnCloudClient {
    agent: Agent,
    base_url: String,
    access_token: Option<String>,
    bearer_token: Option<String>,
}

#[derive(Deserialize)]
struct RandomCodeResp {
    #[serde(default)]
    success: bool,
    #[serde(rename = "randomCode", default)]
    random_code: String,
    #[serde(default)]
    timestamp: serde_json::Value,
    #[serde(rename = "errorMsg", default)]
    error_msg: Option<String>,
}

#[derive(Deserialize)]
struct LoginResp {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    token: String,
    #[serde(rename = "errorMsg", default)]
    error_msg: Option<String>,
}

#[derive(Deserialize)]
struct ListQueryResp {
    #[serde(default)]
    success: bool,
    #[serde(rename = "userFileVOList", default)]
    items: Vec<CloudItem>,
    #[serde(rename = "errorMsg", default)]
    error_msg: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct CloudItem {
    id: serde_json::Value,
    #[serde(rename = "fileName", default)]
    file_name: String,
    #[serde(rename = "isFolder", default)]
    is_folder: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    md5: Option<String>,
    #[serde(rename = "updateTime", default)]
    update_time: Option<serde_json::Value>,
}

impl CloudItem {
    fn id_string(&self) -> String {
        match &self.id {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            other => other.to_string(),
        }
    }

    fn id_i64(&self) -> i64 {
        match &self.id {
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
            serde_json::Value::String(s) => s.parse::<i64>().unwrap_or(0),
            _ => 0,
        }
    }

    fn is_dir(&self) -> bool {
        self.is_folder.eq_ignore_ascii_case("Y") || self.is_folder == "1"
    }

    fn updated_str(&self) -> String {
        match &self.update_time {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }
}

#[derive(Deserialize)]
struct DownloadUrlResp {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    url: String,
    #[serde(rename = "errorMsg", default)]
    error_msg: Option<String>,
}

#[derive(Deserialize)]
struct FolderAddResp {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(rename = "errorMsg", default)]
    error_msg: Option<String>,
}

#[derive(Deserialize)]
struct UploadApplyResp {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    url: String,
    #[serde(rename = "innerName", default)]
    inner_name: Option<String>,
    #[serde(rename = "s3Authorization", default)]
    s3_authorization: Option<String>,
    #[serde(rename = "xamzDate", default)]
    xamz_date: Option<String>,
    #[serde(rename = "errorMsg", default)]
    error_msg: Option<String>,
}

#[derive(Deserialize)]
struct BasicResp {
    #[serde(default)]
    success: bool,
    #[serde(rename = "errorMsg", default)]
    error_msg: Option<String>,
}

impl SnCloudClient {
    pub fn connect(profile: &SyncProfile) -> Result<Self> {
        let base_url = profile.resolved_base_url();
        if base_url.is_empty() {
            bail!("profile `base_url` is required (e.g. https://sn.yourdomain.com)");
        }

        let tls = TlsConfig::builder()
            .disable_verification(!profile.verify_tls)
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(60)))
            .build();
        let agent: Agent = config.into();

        let (account, password, token) = profile.resolved_credentials();

        let mut client = Self {
            agent,
            base_url,
            access_token: None,
            bearer_token: None,
        };

        // Open Private Cloud (`allenporter/supernote`) accepts static `sn_...` Bearer tokens.
        if let Some(tok) = token
            && !tok.is_empty()
        {
            if profile.kind == ProfileKind::OpenPrivateCloud || tok.starts_with("sn_") {
                client.bearer_token = Some(tok);
            } else {
                client.access_token = Some(tok);
            }
            return Ok(client);
        }

        let account = account.context(
            "missing `account` (set in config.toml or via SN_ACCOUNT environment variable)",
        )?;
        let password = password.context(
            "missing `password` (set in config.toml or via SN_PASSWORD environment variable)",
        )?;

        client.login(&account, &password)?;
        Ok(client)
    }

    fn api_url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/api") {
            format!("{base}{path}")
        } else {
            format!("{base}/api{path}")
        }
    }

    fn resolve_download_url(&self, raw_url: &str) -> String {
        if raw_url.starts_with("http://") || raw_url.starts_with("https://") {
            raw_url.to_string()
        } else {
            let base = self.base_url.trim_end_matches('/');
            let rel = raw_url.trim_start_matches('/');
            format!("{base}/{rel}")
        }
    }

    fn post_json<B: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        endpoint: &str,
        body: &B,
    ) -> Result<R> {
        let url = self.api_url(endpoint);
        let mut req = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("User-Agent", USER_AGENT);

        if let Some(tok) = &self.access_token {
            req = req.header("x-access-token", tok);
        }
        if let Some(bearer) = &self.bearer_token {
            req = req.header("Authorization", &format!("Bearer {bearer}"));
        }

        let mut resp = req
            .send_json(body)
            .with_context(|| format!("HTTP POST {url} failed"))?;
        let parsed: R = resp
            .body_mut()
            .read_json()
            .with_context(|| format!("failed to parse JSON response from {url}"))?;
        Ok(parsed)
    }

    /// Two-step Ratta Supernote Cloud / Official Private Cloud challenge-response login.
    fn login(&mut self, account: &str, password: &str) -> Result<()> {
        let rc_payload = serde_json::json!({
            "countryCode": "1",
            "account": account,
        });
        let rc: RandomCodeResp =
            self.post_json("/official/user/query/random/code", &rc_payload)?;
        if !rc.success {
            bail!(
                "random code request failed: {}",
                rc.error_msg.unwrap_or_else(|| "unknown error".to_string())
            );
        }

        let salted = hash_supernote_password(password, &rc.random_code);
        let login_payload = serde_json::json!({
            "countryCode": 1,
            "account": account,
            "password": salted,
            "browser": "Chrome107",
            "equipment": "1",
            "loginMethod": "1",
            "timestamp": rc.timestamp,
            "language": "en",
        });

        let login: LoginResp =
            self.post_json("/official/user/account/login/new", &login_payload)?;
        if !login.success || login.token.is_empty() {
            bail!(
                "Supernote login failed: {}",
                login
                    .error_msg
                    .unwrap_or_else(|| "invalid credentials".to_string())
            );
        }

        self.access_token = Some(login.token);
        Ok(())
    }

    fn list_dir(&self, dir_id: i64) -> Result<Vec<CloudItem>> {
        let mut all = Vec::new();
        let mut page_no = 1u32;
        loop {
            let payload = serde_json::json!({
                "directoryId": dir_id,
                "pageNo": page_no,
                "pageSize": 100,
                "order": "time",
                "sequence": "desc",
            });
            let resp: ListQueryResp = self.post_json("/file/list/query", &payload)?;
            if !resp.success {
                bail!(
                    "list query failed for directory {dir_id}: {}",
                    resp.error_msg.unwrap_or_else(|| "unknown error".to_string())
                );
            }
            let count = resp.items.len();
            all.extend(resp.items);
            if count < 100 {
                break;
            }
            page_no += 1;
        }
        Ok(all)
    }

    fn resolve_dir_id(&self, remote_path: &str, create_missing: bool) -> Result<i64> {
        let trimmed = remote_path.trim().trim_matches('/');
        if trimmed.is_empty() {
            return Ok(0);
        }

        let mut current_id = 0i64;
        for segment in trimmed.split('/').filter(|s| !s.is_empty()) {
            let items = self.list_dir(current_id)?;
            if let Some(found) = items
                .iter()
                .find(|it| it.is_dir() && it.file_name == segment)
            {
                current_id = found.id_i64();
            } else if create_missing {
                let payload = serde_json::json!({
                    "directoryId": current_id,
                    "fileName": segment,
                });
                let added: FolderAddResp = self.post_json("/file/folder/add", &payload)?;
                if !added.success {
                    bail!(
                        "failed to create remote folder `{segment}`: {}",
                        added.error_msg.unwrap_or_else(|| "unknown error".to_string())
                    );
                }
                if let Some(id_val) = added.id {
                    current_id = match id_val {
                        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
                        serde_json::Value::String(s) => s.parse::<i64>().unwrap_or(0),
                        _ => 0,
                    };
                } else {
                    // Re-query directory if server didn't echo the newly created folder id
                    let refreshed = self.list_dir(current_id)?;
                    current_id = refreshed
                        .iter()
                        .find(|it| it.is_dir() && it.file_name == segment)
                        .map(CloudItem::id_i64)
                        .context("created remote folder not found on refresh")?;
                }
            } else {
                bail!("remote directory segment `{segment}` not found in `{remote_path}`");
            }
        }
        Ok(current_id)
    }

    /// Recursively lists all remote files starting at `remote_root`.
    pub fn list_recursive(&self, remote_root: &str) -> Result<Vec<RemoteEntry>> {
        let root_id = self.resolve_dir_id(remote_root, false)?;
        let mut entries = Vec::new();
        self.walk_dir(root_id, "", &mut entries)?;
        Ok(entries)
    }

    fn walk_dir(&self, dir_id: i64, prefix: &str, out: &mut Vec<RemoteEntry>) -> Result<()> {
        let items = self.list_dir(dir_id)?;
        for item in items {
            let rel = if prefix.is_empty() {
                item.file_name.clone()
            } else {
                format!("{prefix}/{}", item.file_name)
            };
            if item.is_dir() {
                self.walk_dir(item.id_i64(), &rel, out)?;
            } else {
                out.push(RemoteEntry {
                    remote_path: rel,
                    id: item.id_string(),
                    size: item.size,
                    fingerprint: item.md5.clone().unwrap_or_else(|| item.updated_str()),
                });
            }
        }
        Ok(())
    }

    /// Downloads a single file by its Supernote Cloud ID.
    pub fn download_file(&self, entry: &RemoteEntry) -> Result<Vec<u8>> {
        let id_val: serde_json::Value = entry
            .id
            .parse::<i64>()
            .map(serde_json::Value::from)
            .unwrap_or_else(|_| serde_json::Value::String(entry.id.clone()));

        let payload = serde_json::json!({
            "id": id_val,
            "type": 0,
        });
        let resp: DownloadUrlResp = self.post_json("/file/download/url", &payload)?;
        if !resp.success || resp.url.is_empty() {
            bail!(
                "download url failed for {}: {}",
                entry.remote_path,
                resp.error_msg.unwrap_or_else(|| "empty url".to_string())
            );
        }

        let dl_url = self.resolve_download_url(&resp.url);
        let mut req = self.agent.get(&dl_url).header("User-Agent", USER_AGENT);
        // If downloading from same origin (e.g. Private Cloud Docker), forward auth headers too
        if dl_url.starts_with(&self.base_url) {
            if let Some(tok) = &self.access_token {
                req = req.header("x-access-token", tok);
            }
            if let Some(bearer) = &self.bearer_token {
                req = req.header("Authorization", &format!("Bearer {bearer}"));
            }
        }

        let mut dl_resp = req
            .call()
            .with_context(|| format!("failed to download {}", entry.remote_path))?;
        let bytes = dl_resp
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD_BYTES)
            .read_to_vec()
            .with_context(|| format!("failed reading download body for {}", entry.remote_path))?;
        Ok(bytes)
    }

    /// Explicitly uploads a local file (e.g., PDF or template) into `remote_dest`.
    pub fn upload_file(&self, local_file: &Path, remote_dest: &str) -> Result<()> {
        let file_name = local_file
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid local filename")?;
        let data = fs::read(local_file)
            .with_context(|| format!("failed to read {}", local_file.display()))?;
        let md5_hex = hex_md5(&data);
        let dir_id = self.resolve_dir_id(remote_dest, true)?;

        let apply_payload = serde_json::json!({
            "directoryId": dir_id,
            "fileName": file_name,
            "md5": md5_hex,
            "size": data.len(),
        });
        let applied: UploadApplyResp = self.post_json("/file/upload/apply", &apply_payload)?;
        if !applied.success || applied.url.is_empty() {
            bail!(
                "upload apply failed for {file_name}: {}",
                applied.error_msg.unwrap_or_else(|| "unknown error".to_string())
            );
        }

        let upload_url = self.resolve_download_url(&applied.url);
        let mut put_req = self.agent.put(&upload_url).header("User-Agent", USER_AGENT);
        if let (Some(auth), Some(date)) = (&applied.s3_authorization, &applied.xamz_date) {
            put_req = put_req
                .header("Authorization", auth)
                .header("x-amz-date", date)
                .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD");
        } else {
            if let Some(tok) = &self.access_token {
                put_req = put_req.header("x-access-token", tok);
            }
            if let Some(bearer) = &self.bearer_token {
                put_req = put_req.header("Authorization", &format!("Bearer {bearer}"));
            }
        }

        put_req
            .send(&data[..])
            .with_context(|| format!("upload PUT to {upload_url} failed"))?;

        let inner_name = applied.inner_name.unwrap_or_else(|| {
            applied
                .url
                .rsplit('/')
                .next()
                .unwrap_or(file_name)
                .to_string()
        });

        let finish_payload = serde_json::json!({
            "directoryId": dir_id,
            "fileName": file_name,
            "fileSize": data.len(),
            "innerName": inner_name,
            "md5": md5_hex,
        });
        let finished: BasicResp = self.post_json("/file/upload/finish", &finish_payload)?;
        if !finished.success {
            bail!(
                "upload finish failed for {file_name}: {}",
                finished
                    .error_msg
                    .unwrap_or_else(|| "unknown error".to_string())
            );
        }

        Ok(())
    }
}

pub fn hash_supernote_password(password: &str, random_code: &str) -> String {
    let md5_hex = hex_md5(password.as_bytes());
    let mut sha = Sha256::new();
    sha.update(md5_hex.as_bytes());
    sha.update(random_code.as_bytes());
    hex_encode(&sha.finalize())
}

fn hex_md5(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}
