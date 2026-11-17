//! Pure-Rust WebDAV client (`PROPFIND` via `quick-xml`, `GET` pull, `PUT` explicit upload).

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use quick_xml::Reader;
use quick_xml::events::Event;
use ureq::Agent;
use ureq::http::Request;
use ureq::tls::TlsConfig;

use crate::config::SyncProfile;
use crate::sync::RemoteEntry;

const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:resourcetype/>
    <d:getcontentlength/>
    <d:getetag/>
    <d:getlastmodified/>
  </d:prop>
</d:propfind>"#;

pub struct WebDavClient {
    agent: Agent,
    base_url: String,
    auth_header: Option<String>,
}

#[derive(Debug, Default)]
struct DavItem {
    href: String,
    is_collection: bool,
    size: u64,
    etag: String,
    last_modified: String,
}

impl WebDavClient {
    pub fn connect(profile: &SyncProfile) -> Result<Self> {
        let base_url = profile.resolved_base_url();
        if base_url.is_empty() {
            bail!("WebDAV profile requires `base_url`");
        }

        let tls = TlsConfig::builder()
            .disable_verification(!profile.verify_tls)
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .allow_non_standard_methods(true)
            .timeout_global(Some(Duration::from_secs(60)))
            .build();
        let agent: Agent = config.into();

        let (account, password, token) = profile.resolved_credentials();
        let auth_header = if let Some(tok) = token
            && !tok.is_empty()
        {
            Some(format!("Bearer {tok}"))
        } else if let Some(user) = account {
            let pass = password.unwrap_or_default();
            let encoded = BASE64.encode(format!("{user}:{pass}").as_bytes());
            Some(format!("Basic {encoded}"))
        } else {
            None
        };

        Ok(Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_header,
        })
    }

    fn full_url(&self, rel: &str) -> String {
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            format!("{}/", self.base_url)
        } else {
            format!("{}/{}", self.base_url, rel)
        }
    }

    fn base_path_prefix(&self) -> String {
        // Extract path component from base_url (e.g. "https://host:8080/dav" -> "/dav")
        let after_scheme = self
            .base_url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&self.base_url);
        match after_scheme.find('/') {
            Some(idx) => after_scheme[idx..].trim_end_matches('/').to_string(),
            None => String::new(),
        }
    }

    pub fn list_recursive(&self, remote_root: &str) -> Result<Vec<RemoteEntry>> {
        let start = remote_root.trim_matches('/').to_string();
        let mut out = Vec::new();
        self.walk_collection(&start, &mut out)?;
        Ok(out)
    }

    fn walk_collection(&self, rel_dir: &str, out: &mut Vec<RemoteEntry>) -> Result<()> {
        let url = self.full_url(rel_dir);
        let mut builder = Request::builder()
            .method("PROPFIND")
            .uri(&url)
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8");
        if let Some(auth) = &self.auth_header {
            builder = builder.header("Authorization", auth);
        }
        let req = builder.body(PROPFIND_BODY.as_bytes())?;
        let mut resp = self
            .agent
            .run(req)
            .with_context(|| format!("WebDAV PROPFIND failed for {url}"))?;
        let xml = resp.body_mut().read_to_string()?;

        let items = parse_propfind_xml(&xml)?;
        let base_prefix = self.base_path_prefix();
        let current_norm = rel_dir.trim_matches('/');

        for item in items {
            let decoded_href = percent_decode(&item.href);
            // Strip scheme+host if server returned absolute URL in <href>
            let href_path = if let Some((_, rest)) = decoded_href.split_once("://") {
                rest.find('/').map(|i| &rest[i..]).unwrap_or("/")
            } else {
                decoded_href.as_str()
            };

            let stripped = href_path
                .strip_prefix(&base_prefix)
                .unwrap_or(href_path)
                .trim_matches('/');

            // Skip self-entry returned by Depth: 1
            if stripped == current_norm || stripped.is_empty() {
                continue;
            }

            if item.is_collection {
                self.walk_collection(stripped, out)?;
            } else {
                let fp = if !item.etag.is_empty() {
                    item.etag
                } else {
                    item.last_modified
                };
                out.push(RemoteEntry {
                    remote_path: stripped.to_string(),
                    id: stripped.to_string(),
                    size: item.size,
                    fingerprint: fp,
                });
            }
        }
        Ok(())
    }

    pub fn download_file(&self, entry: &RemoteEntry) -> Result<Vec<u8>> {
        let url = self.full_url(&entry.remote_path);
        let mut req = self.agent.get(&url);
        if let Some(auth) = &self.auth_header {
            req = req.header("Authorization", auth);
        }
        let mut resp = req
            .call()
            .with_context(|| format!("WebDAV GET failed for {url}"))?;
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD_BYTES)
            .read_to_vec()?;
        Ok(bytes)
    }

    pub fn upload_file(&self, local_file: &Path, remote_dest: &str) -> Result<()> {
        let file_name = local_file
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid local filename")?;
        let data = fs::read(local_file)?;
        let dest_clean = remote_dest.trim_matches('/');
        let rel = if dest_clean.is_empty() {
            file_name.to_string()
        } else {
            format!("{dest_clean}/{file_name}")
        };
        let url = self.full_url(&rel);
        let mut req = self.agent.put(&url);
        if let Some(auth) = &self.auth_header {
            req = req.header("Authorization", auth);
        }
        req.send(&data[..])
            .with_context(|| format!("WebDAV PUT failed for {url}"))?;
        Ok(())
    }
}

fn local_name(bytes: &[u8]) -> &[u8] {
    match bytes.iter().rposition(|&b| b == b':') {
        Some(pos) => &bytes[pos + 1..],
        None => bytes,
    }
}

fn parse_propfind_xml(xml: &str) -> Result<Vec<DavItem>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut items = Vec::new();
    let mut current: Option<DavItem> = None;
    let mut active_tag: Option<Vec<u8>> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let lname = local_name(e.name().as_ref()).to_ascii_lowercase();
                if lname == b"response" {
                    current = Some(DavItem::default());
                } else if lname == b"collection" {
                    if let Some(item) = current.as_mut() {
                        item.is_collection = true;
                    }
                } else {
                    active_tag = Some(lname);
                }
            }
            Event::Empty(e) => {
                let lname = local_name(e.name().as_ref()).to_ascii_lowercase();
                if lname == b"collection"
                    && let Some(item) = current.as_mut()
                {
                    item.is_collection = true;
                }
            }
            Event::Text(e) => {
                if let (Some(item), Some(tag)) = (current.as_mut(), active_tag.as_deref()) {
                    let text = String::from_utf8_lossy(e.as_ref()).trim().to_string();
                    match tag {
                        b"href" => item.href = text,
                        b"getcontentlength" => {
                            if let Ok(sz) = text.parse::<u64>() {
                                item.size = sz;
                            }
                        }
                        b"getetag" => item.etag = text.trim_matches('"').to_string(),
                        b"getlastmodified" => item.last_modified = text,
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                let lname = local_name(e.name().as_ref()).to_ascii_lowercase();
                if lname == b"response" {
                    if let Some(item) = current.take()
                        && !item.href.is_empty()
                    {
                        items.push(item);
                    }
                } else {
                    active_tag = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(items)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
