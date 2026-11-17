use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use sn_tui::config::{ProfileKind, SyncProfile};
use sn_tui::sync::sncloud::hash_supernote_password;
use sn_tui::sync::watcher::WorkspaceWatcher;
use sn_tui::sync::{pull_sync, upload_file};

struct HttpReq {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut std::net::TcpStream) -> Option<HttpReq> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end;
    loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
    }

    let head_str = String::from_utf8_lossy(&raw[..header_end]);
    let mut lines = head_str.lines();
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let kl = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            if kl == "content-length" {
                content_length = val.parse().unwrap_or(0);
            }
            headers.push((kl, val));
        }
    }

    let mut body = raw[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Some(HttpReq {
        method,
        path,
        headers,
        body,
    })
}

fn send_http(stream: &mut std::net::TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

#[test]
fn private_cloud_challenge_login_incremental_pull_and_explicit_upload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let expected_pw_hash = hash_supernote_password("s3cr3t!", "RC123456");
    let uploaded_bytes: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let uploaded_clone = Arc::clone(&uploaded_bytes);
    let note_payload = b"noteSN_FILE_VER_20230015FAKEPAYLOAD".to_vec();
    let note_clone = note_payload.clone();

    let server_handle = thread::spawn(move || {
        for stream in listener.incoming().take(32) {
            let Ok(mut stream) = stream else {
                break;
            };
            let Some(req) = read_http_request(&mut stream) else {
                continue;
            };

            let token_hdr = req
                .headers
                .iter()
                .find(|(k, _)| k == "x-access-token")
                .map(|(_, v)| v.as_str());

            match (req.method.as_str(), req.path.as_str()) {
                ("POST", "/api/official/user/query/random/code") => {
                    let body = serde_json::json!({
                        "success": true,
                        "randomCode": "RC123456",
                        "timestamp": "1700000000000"
                    })
                    .to_string();
                    send_http(&mut stream, "200 OK", "application/json", body.as_bytes());
                }
                ("POST", "/api/official/user/account/login/new") => {
                    let json: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                    assert_eq!(json["password"].as_str(), Some(expected_pw_hash.as_str()));
                    let body = serde_json::json!({
                        "success": true,
                        "token": "tok_test_private_cloud"
                    })
                    .to_string();
                    send_http(&mut stream, "200 OK", "application/json", body.as_bytes());
                }
                ("POST", "/api/file/list/query") => {
                    assert_eq!(token_hdr, Some("tok_test_private_cloud"));
                    let json: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                    let dir_id = json["directoryId"].as_i64().unwrap_or(0);
                    let list = if dir_id == 0 {
                        serde_json::json!([
                            { "id": 10, "fileName": "Note", "isFolder": "Y", "size": 0 },
                            { "id": 20, "fileName": "Document", "isFolder": "Y", "size": 0 }
                        ])
                    } else if dir_id == 10 {
                        serde_json::json!([
                            {
                                "id": 101,
                                "fileName": "daily.note",
                                "isFolder": "N",
                                "size": note_clone.len(),
                                "md5": "deadbeef01"
                            }
                        ])
                    } else {
                        serde_json::json!([])
                    };
                    let body = serde_json::json!({
                        "success": true,
                        "userFileVOList": list
                    })
                    .to_string();
                    send_http(&mut stream, "200 OK", "application/json", body.as_bytes());
                }
                ("POST", "/api/file/download/url") => {
                    assert_eq!(token_hdr, Some("tok_test_private_cloud"));
                    let body = serde_json::json!({
                        "success": true,
                        "url": "/dl/daily.note"
                    })
                    .to_string();
                    send_http(&mut stream, "200 OK", "application/json", body.as_bytes());
                }
                ("GET", "/dl/daily.note") => {
                    send_http(&mut stream, "200 OK", "application/octet-stream", &note_clone);
                }
                ("POST", "/api/file/upload/apply") => {
                    assert_eq!(token_hdr, Some("tok_test_private_cloud"));
                    let body = serde_json::json!({
                        "success": true,
                        "url": "/upload/handbook.pdf",
                        "innerName": "handbook_inner.pdf"
                    })
                    .to_string();
                    send_http(&mut stream, "200 OK", "application/json", body.as_bytes());
                }
                ("PUT", "/upload/handbook.pdf") => {
                    *uploaded_clone.lock().unwrap() = Some(req.body.clone());
                    send_http(&mut stream, "200 OK", "text/plain", b"ok");
                }
                ("POST", "/api/file/upload/finish") => {
                    let body = serde_json::json!({ "success": true }).to_string();
                    send_http(&mut stream, "200 OK", "application/json", body.as_bytes());
                    break;
                }
                _ => {
                    send_http(&mut stream, "404 Not Found", "text/plain", b"not found");
                }
            }
        }
    });

    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    let cache = dir.path().join("cache");

    let profile = SyncProfile {
        kind: ProfileKind::PrivateCloud,
        base_url: Some(base_url),
        account: Some("user@example.com".to_string()),
        password: Some("s3cr3t!".to_string()),
        token: None,
        remote_root: "/".to_string(),
        verify_tls: false,
    };

    // 1st pull downloads Note/daily.note
    let rep1 = pull_sync("private", &profile, &ws, &cache, false, |_| {}).expect("first pull");
    assert_eq!(rep1.downloaded, 1);
    assert_eq!(rep1.skipped, 0);
    assert_eq!(fs::read(ws.join("Note").join("daily.note")).unwrap(), note_payload);

    // 2nd pull skips unchanged Note/daily.note
    let rep2 = pull_sync("private", &profile, &ws, &cache, false, |_| {}).expect("second pull");
    assert_eq!(rep2.downloaded, 0);
    assert_eq!(rep2.skipped, 1);

    // Explicit upload sends local PDF to /Document
    let local_pdf = dir.path().join("handbook.pdf");
    let pdf_bytes = b"%PDF-1.4\nhello supernote\n%%EOF".to_vec();
    fs::write(&local_pdf, &pdf_bytes).expect("write pdf");
    upload_file(&profile, &local_pdf, "/Document").expect("upload pdf");

    server_handle.join().expect("server thread");
    assert_eq!(uploaded_bytes.lock().unwrap().as_deref(), Some(pdf_bytes.as_slice()));
}

#[test]
fn webdav_propfind_pull_and_put_upload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}/dav");

    let payload = b"webdav-note-bytes".to_vec();
    let payload_clone = payload.clone();
    let uploaded: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let uploaded_clone = Arc::clone(&uploaded);

    let server = thread::spawn(move || {
        for stream in listener.incoming().take(10) {
            let Ok(mut stream) = stream else {
                break;
            };
            let Some(req) = read_http_request(&mut stream) else {
                continue;
            };
            match (req.method.as_str(), req.path.as_str()) {
                ("PROPFIND", "/dav/") => {
                    let xml = format!(
                        r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/inbox.note</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/>
      <D:getcontentlength>{}</D:getcontentlength>
      <D:getetag>"etag-v1"</D:getetag>
    </D:prop></D:propstat>
  </D:response>
</D:multistatus>"#,
                        payload_clone.len()
                    );
                    send_http(&mut stream, "207 Multi-Status", "application/xml", xml.as_bytes());
                }
                ("GET", "/dav/inbox.note") => {
                    send_http(&mut stream, "200 OK", "application/octet-stream", &payload_clone);
                }
                ("PUT", "/dav/Document/new.pdf") => {
                    *uploaded_clone.lock().unwrap() = Some(req.body.clone());
                    send_http(&mut stream, "201 Created", "text/plain", b"created");
                    break;
                }
                _ => send_http(&mut stream, "404 Not Found", "text/plain", b"not found"),
            }
        }
    });

    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    let cache = dir.path().join("cache");
    let profile = SyncProfile {
        kind: ProfileKind::Webdav,
        base_url: Some(base_url),
        account: Some("alice".to_string()),
        password: Some("pass".to_string()),
        token: None,
        remote_root: "/".to_string(),
        verify_tls: false,
    };

    let rep1 = pull_sync("dav", &profile, &ws, &cache, false, |_| {}).expect("webdav pull 1");
    assert_eq!(rep1.downloaded, 1);
    assert_eq!(fs::read(ws.join("inbox.note")).unwrap(), payload);

    let rep2 = pull_sync("dav", &profile, &ws, &cache, false, |_| {}).expect("webdav pull 2");
    assert_eq!(rep2.downloaded, 0);
    assert_eq!(rep2.skipped, 1);

    let local_pdf = dir.path().join("new.pdf");
    fs::write(&local_pdf, b"pdf-content").unwrap();
    upload_file(&profile, &local_pdf, "/Document").expect("webdav put");

    server.join().expect("server thread");
    assert_eq!(uploaded.lock().unwrap().as_deref(), Some(b"pdf-content".as_slice()));
}

#[test]
fn filesystem_watcher_detects_new_note_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut watcher = WorkspaceWatcher::start(dir.path()).expect("start watcher");

    let target = dir.path().join("incoming.note");
    fs::write(&target, b"hello").expect("write note");

    let mut detected = false;
    for _ in 0..30 {
        let changed = watcher.poll_changed();
        if changed.iter().any(|p| p.ends_with("incoming.note")) {
            detected = true;
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }
    assert!(detected, "expected watcher to report incoming.note");
}
