//! Gmail integration. OAuth 2.0 with locally-cached refresh tokens, so
//! the first run opens a browser for auth and every subsequent process
//! reuses the cached token. Tools cover search, read, send, draft,
//! unread-count, mark-read, archive.
//!
//! Setup the user does once:
//! ```text
//! AEGIS_GMAIL_CLIENT_ID=...
//! AEGIS_GMAIL_CLIENT_SECRET=...
//! ```
//! placed in `.env`. First `cargo run` opens a browser and writes the
//! refresh token to `~/.config/aegis/gmail_token.json` (mode 0600 on Unix).

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::sync::OnceLock;

const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

/// OAuth scopes the user grants. `modify` covers archive/mark-read,
/// `send`/`compose` cover outbound. Read-only is included so a future
/// user who only wants triage can downscope.
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.compose",
    "https://www.googleapis.com/auth/gmail.send",
];

/// True iff both OAuth env vars are set. Hides Gmail tools from Claude's
/// tools array when false so the agent doesn't call something that would
/// fail at runtime.
pub fn is_available() -> bool {
    dotenvy::dotenv().ok();
    let id = std::env::var("AEGIS_GMAIL_CLIENT_ID").unwrap_or_default();
    let secret = std::env::var("AEGIS_GMAIL_CLIENT_SECRET").unwrap_or_default();
    !id.is_empty() && !secret.is_empty()
}

/// Tool schemas this integration adds to Claude's tools array.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "gmail_search",
            "description": "Search the user's Gmail inbox via the Gmail API. \
                Works regardless of what window is visible on screen. \
                ALWAYS use this when the user asks about their email/mail/inbox, \
                even if no email client is open. \
                Gmail query syntax: 'from:alice', 'subject:report', 'is:unread', \
                'has:attachment', 'newer_than:7d', etc. Combine with spaces. \
                Returns a JSON array of {id, threadId, from, subject, snippet, date}.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Gmail search query." },
                    "max_results": {
                        "type": "integer",
                        "description": "Max messages to return (default 10, cap 25).",
                        "minimum": 1, "maximum": 25
                    }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "gmail_read",
            "description": "Fetch the full content of one Gmail message by ID. \
                Use after gmail_search returns hits, with the id from a result. \
                Returns from/to/cc/subject/date/body.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Gmail message ID from gmail_search." }
                },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "gmail_send",
            "description": "Send a real email from the user's Gmail. \
                Goes out immediately. For drafts use gmail_draft instead.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "to":      { "type": "string", "description": "Recipient address." },
                    "subject": { "type": "string", "description": "Subject line." },
                    "body":    { "type": "string", "description": "Plain-text body." },
                    "cc":      { "type": "string", "description": "CC address (optional)." }
                },
                "required": ["to", "subject", "body"]
            }
        }),
        serde_json::json!({
            "name": "gmail_draft",
            "description": "Save an email as a Gmail draft without sending. \
                Use when the user says 'draft', 'compose', or wants to review before sending.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "to":      { "type": "string", "description": "Recipient address." },
                    "subject": { "type": "string", "description": "Subject line." },
                    "body":    { "type": "string", "description": "Plain-text body." },
                    "cc":      { "type": "string", "description": "CC address (optional)." }
                },
                "required": ["to", "subject", "body"]
            }
        }),
        serde_json::json!({
            "name": "gmail_unread_count",
            "description": "Return the user's Gmail INBOX unread count. \
                Works regardless of what is on screen. \
                Use for 'do I have new mail?' / 'how many unread?' queries.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "gmail_mark_read",
            "description": "Mark a Gmail message as read (remove the UNREAD label).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Gmail message ID." }
                },
                "required": ["id"]
            }
        }),
        serde_json::json!({
            "name": "gmail_archive",
            "description": "Archive a Gmail message (remove from INBOX, keeps in All Mail).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Gmail message ID." }
                },
                "required": ["id"]
            }
        }),
    ]
}

/// Returns `Some(json)` if this integration owned the tool, `None`
/// otherwise. `json` is the Gmail API response (or `{"error": "..."}`).
/// Blocks the caller; each command runs through the cached tokio runtime.
pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "gmail_search" => Some(block(cmd_search(input))),
        "gmail_read" => Some(block(cmd_read(input))),
        "gmail_send" => Some(block(cmd_send(input))),
        "gmail_draft" => Some(block(cmd_draft(input))),
        "gmail_unread_count" => Some(block(cmd_unread_count())),
        "gmail_mark_read" => Some(block(cmd_modify(
            input,
            serde_json::json!({"removeLabelIds": ["UNREAD"]}),
        ))),
        "gmail_archive" => Some(block(cmd_modify(
            input,
            serde_json::json!({"removeLabelIds": ["INBOX"]}),
        ))),
        _ => None,
    }
}

/// Cached on first successful fetch of /profile. The Gmail address of the
/// authenticated user is stable for the lifetime of the OAuth grant, so we
/// only need to ask Google once per process. Subsequent reads are free.
static USER_EMAIL: OnceLock<String> = OnceLock::new();

/// The authenticated user's own Gmail address (e.g. "alice@gmail.com").
/// Returns `None` if Gmail isn't configured or the profile fetch failed.
/// Blocks once on first call to hit the Gmail profile endpoint; cached
/// thereafter so callers can use it in hot paths without worrying about
/// per-call cost.
pub fn user_email() -> Option<String> {
    if let Some(e) = USER_EMAIL.get() {
        return Some(e.clone());
    }
    if !is_available() {
        return None;
    }
    // The async fn writes to USER_EMAIL on success and returns its result
    // as a status string we discard here. We just want the side effect.
    let _ = block(cmd_fetch_user_email());
    USER_EMAIL.get().cloned()
}

async fn cmd_fetch_user_email() -> String {
    let t = std::time::Instant::now();
    let token = match bearer().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[gmail] cmd_fetch_user_email auth failed: {e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e));
        }
    };
    let resp = http()
        .get(format!("{GMAIL_BASE}/profile"))
        .bearer_auth(&token)
        .send()
        .await;
    let out = match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            match body["emailAddress"].as_str() {
                Some(email) => {
                    let _ = USER_EMAIL.set(email.to_string());
                    format!(r#"{{"emailAddress":{}}}"#, serde_json::json!(email))
                }
                None => format!(
                    r#"{{"error":"profile response missing emailAddress: {}"}}"#,
                    body
                ),
            }
        }
        Ok(r) => {
            let status = r.status().as_u16();
            format!(r#"{{"error":"HTTP {status} on /profile"}}"#)
        }
        Err(e) => format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string())),
    };
    eprintln!("[gmail] cmd_fetch_user_email TOTAL {:?}", t.elapsed());
    out
}

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn rt() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("[integration:gmail] failed to build tokio runtime")
    })
}

/// Bridge sync callers (the integration `dispatch` API) to async Gmail calls.
/// When invoked from inside the agent loop we are already on a tokio worker
/// thread; nested `block_on` panics, so use `block_in_place` + the current
/// `Handle`. Outside any runtime (test binaries, future sync callers) fall
/// back to a private multi-thread runtime.
fn block<F>(fut: F) -> F::Output
where
    F: std::future::Future,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => rt().block_on(fut),
    }
}

type HyperConnector =
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;
type GmailAuth = yup_oauth2::authenticator::Authenticator<HyperConnector>;

static AUTH: OnceLock<GmailAuth> = OnceLock::new();

async fn init_auth() -> Result<&'static GmailAuth, String> {
    if let Some(a) = AUTH.get() {
        return Ok(a);
    }

    // rustls 0.23+ needs a CryptoProvider installed before the first TLS
    // handshake. install_default returns Err on the second call; that's
    // fine since we only care that one is set.
    let _ = rustls::crypto::ring::default_provider().install_default();

    dotenvy::dotenv().ok();
    let client_id = std::env::var("AEGIS_GMAIL_CLIENT_ID")
        .map_err(|_| "[integration:gmail] AEGIS_GMAIL_CLIENT_ID not set".to_string())?;
    let client_secret = std::env::var("AEGIS_GMAIL_CLIENT_SECRET")
        .map_err(|_| "[integration:gmail] AEGIS_GMAIL_CLIENT_SECRET not set".to_string())?;

    let config_dir = dirs::config_dir()
        .ok_or_else(|| "[integration:gmail] dirs::config_dir() returned None".to_string())?;
    let aegis_dir = config_dir.join("aegis");
    std::fs::create_dir_all(&aegis_dir)
        .map_err(|e| format!("[integration:gmail] could not create config dir: {e}"))?;
    let token_path = aegis_dir.join("gmail_token.json");

    let secret = yup_oauth2::ApplicationSecret {
        client_id,
        client_secret,
        auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
        token_uri: "https://oauth2.googleapis.com/token".to_string(),
        redirect_uris: vec!["http://localhost".to_string()],
        ..Default::default()
    };

    let connector: HyperConnector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| format!("[integration:gmail] TLS native roots: {e}"))?
        .https_or_http()
        .enable_http2()
        .build();
    let hyper_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    let new_auth: GmailAuth = yup_oauth2::InstalledFlowAuthenticator::with_client(
        secret,
        yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
        yup_oauth2::client::CustomHyperClientBuilder::from(hyper_client),
    )
    .persist_tokens_to_disk(&token_path)
    .build()
    .await
    .map_err(|e| format!("[integration:gmail] auth build failed: {e}"))?;

    // Lock the token file down to owner-read-only on Unix. On Windows the
    // analogue is ACLs and the per-user AppData dir is already private to
    // the current user, so we leave the file's default permissions alone.
    #[cfg(unix)]
    if token_path.exists() {
        let _ = std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600));
    }

    let _ = AUTH.set(new_auth);
    Ok(AUTH.get().expect("just set"))
}

async fn bearer() -> Result<String, String> {
    let auth: &GmailAuth = init_auth().await?;
    let token = auth
        .token(SCOPES)
        .await
        .map_err(|e| format!("[integration:gmail] token fetch failed: {e}"))?;
    token
        .token()
        .map(str::to_string)
        .ok_or_else(|| "[integration:gmail] token had no value".to_string())
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

async fn cmd_search(input: &serde_json::Value) -> String {
    let t_total = std::time::Instant::now();
    let query = match input["query"].as_str() {
        Some(q) => q,
        None => return r#"{"error":"missing 'query' field"}"#.to_string(),
    };
    let max = input["max_results"]
        .as_u64()
        .map(|n| n.min(25))
        .unwrap_or(10);
    eprintln!("[gmail] cmd_search query='{query}' max={max}");

    let t_token = std::time::Instant::now();
    let token = match bearer().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e));
        }
    };
    eprintln!("[gmail] cmd_search bearer in {:?}", t_token.elapsed());

    let t_list = std::time::Instant::now();
    let list_resp: Result<reqwest::Response, reqwest::Error> = http()
        .get(format!("{GMAIL_BASE}/messages"))
        .bearer_auth(&token)
        .query(&[("q", query), ("maxResults", max.to_string().as_str())])
        .send()
        .await;

    let list_body: serde_json::Value = match list_resp {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(j) => j,
            Err(e) => return format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string())),
        },
        Ok(r) => {
            let status = r.status().as_u16();
            let text: String = r.text().await.unwrap_or_default();
            eprintln!("[integration:gmail] messages.list HTTP {status}: {text}");
            return format!(r#"{{"error":"HTTP {status}"}}"#);
        }
        Err(e) => {
            eprintln!("[integration:gmail] messages.list request failed: {e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string()));
        }
    };

    eprintln!("[gmail] cmd_search messages.list in {:?}", t_list.elapsed());
    let stubs: Vec<serde_json::Value> = match list_body["messages"].as_array() {
        Some(a) => a.clone(),
        None => return "[]".to_string(),
    };
    eprintln!(
        "[gmail] cmd_search list returned {} stubs, fetching metadata...",
        stubs.len()
    );

    let t_gets = std::time::Instant::now();
    // Fire all messages.get calls concurrently. Each get is independent
    // I/O; serializing them paid N x ~180ms. Concurrent we pay max(180ms).
    let fetches = stubs.into_iter().filter_map(|stub| {
        let id = stub["id"].as_str()?.to_string();
        let thread_id = stub["threadId"].as_str().unwrap_or("").to_string();
        let token = token.clone();
        Some(async move {
            let t_one = std::time::Instant::now();
            let msg_resp = http()
                .get(format!("{GMAIL_BASE}/messages/{id}"))
                .bearer_auth(&token)
                .query(&[
                    ("format", "metadata"),
                    ("metadataHeaders", "From"),
                    ("metadataHeaders", "Subject"),
                    ("metadataHeaders", "Date"),
                ])
                .send()
                .await;

            let msg: serde_json::Value = match msg_resp {
                Ok(r) if r.status().is_success() => match r.json().await {
                    Ok(j) => j,
                    Err(e) => {
                        eprintln!("[integration:gmail] messages.get parse failed: {e}");
                        return None;
                    }
                },
                Ok(r) => {
                    eprintln!("[integration:gmail] messages.get HTTP {}", r.status());
                    return None;
                }
                Err(e) => {
                    eprintln!("[integration:gmail] messages.get request failed: {e}");
                    return None;
                }
            };

            let headers = parse_headers(&msg);
            let out = serde_json::json!({
                "id": id,
                "threadId": thread_id,
                "from": headers.get("from").cloned().unwrap_or_default(),
                "subject": headers.get("subject").cloned().unwrap_or_default(),
                "date": headers.get("date").cloned().unwrap_or_default(),
                "snippet": msg["snippet"].as_str().unwrap_or_default(),
            });
            eprintln!(
                "[gmail] cmd_search messages.get {} in {:?}",
                id,
                t_one.elapsed()
            );
            Some(out)
        })
    });

    let results: Vec<serde_json::Value> = futures_util::future::join_all(fetches)
        .await
        .into_iter()
        .flatten()
        .collect();
    eprintln!(
        "[gmail] cmd_search all metadata gets done in {:?} ({} results, parallel)",
        t_gets.elapsed(),
        results.len()
    );
    eprintln!("[gmail] cmd_search TOTAL {:?}", t_total.elapsed());

    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}

async fn cmd_read(input: &serde_json::Value) -> String {
    let t_total = std::time::Instant::now();
    let id = match input["id"].as_str() {
        Some(id) => id,
        None => return r#"{"error":"missing 'id' field"}"#.to_string(),
    };
    eprintln!("[gmail] cmd_read id={id}");

    let token = match bearer().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e));
        }
    };

    let t_http = std::time::Instant::now();
    let resp: Result<reqwest::Response, reqwest::Error> = http()
        .get(format!("{GMAIL_BASE}/messages/{id}"))
        .bearer_auth(&token)
        .query(&[("format", "full")])
        .send()
        .await;

    let msg: serde_json::Value = match resp {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(j) => j,
            Err(e) => return format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string())),
        },
        Ok(r) => {
            let status = r.status().as_u16();
            return format!(r#"{{"error":"HTTP {status}"}}"#);
        }
        Err(e) => {
            eprintln!("[integration:gmail] messages.get failed: {e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string()));
        }
    };
    eprintln!(
        "[gmail] cmd_read messages.get in {:?} (full mode)",
        t_http.elapsed()
    );

    let headers = parse_headers(&msg);
    if std::env::var("AEGIS_GMAIL_DEBUG").is_ok() {
        eprintln!(
            "[gmail debug] payload.mimeType={:?} payload.body.size={:?} parts.len={:?}",
            msg["payload"]["mimeType"].as_str(),
            msg["payload"]["body"]["size"].as_i64(),
            msg["payload"]["parts"].as_array().map(|a| a.len()),
        );
    }
    let body = extract_body(&msg["payload"]);

    let out = serde_json::to_string(&serde_json::json!({
        "id": id,
        "from":    headers.get("from").cloned().unwrap_or_default(),
        "to":      headers.get("to").cloned().unwrap_or_default(),
        "cc":      headers.get("cc").cloned().unwrap_or_default(),
        "subject": headers.get("subject").cloned().unwrap_or_default(),
        "date":    headers.get("date").cloned().unwrap_or_default(),
        "body":    body,
    }))
    .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string());
    eprintln!(
        "[gmail] cmd_read TOTAL {:?} (body {} chars)",
        t_total.elapsed(),
        out.len()
    );
    out
}

async fn cmd_send(input: &serde_json::Value) -> String {
    let t_total = std::time::Instant::now();
    eprintln!(
        "[gmail] cmd_send to={:?} subject={:?}",
        input["to"].as_str(),
        input["subject"].as_str()
    );
    let raw = match build_raw_b64(input) {
        Ok(r) => r,
        Err(e) => return format!(r#"{{"error":{}}}"#, serde_json::json!(e)),
    };

    let token = match bearer().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e));
        }
    };

    let t_http = std::time::Instant::now();
    let resp: Result<reqwest::Response, reqwest::Error> = http()
        .post(format!("{GMAIL_BASE}/messages/send"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"raw": raw}))
        .send()
        .await;

    let out = match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            serde_json::to_string(&serde_json::json!({
                "status": "sent",
                "id": body["id"].as_str().unwrap_or_default(),
            }))
            .unwrap_or_else(|_| r#"{"status":"sent"}"#.to_string())
        }
        Ok(r) => {
            let status = r.status().as_u16();
            let text: String = r.text().await.unwrap_or_default();
            eprintln!("[integration:gmail] messages.send HTTP {status}: {text}");
            format!(r#"{{"error":"HTTP {status}"}}"#)
        }
        Err(e) => {
            eprintln!("[integration:gmail] messages.send failed: {e}");
            format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string()))
        }
    };
    eprintln!(
        "[gmail] cmd_send TOTAL {:?} (http {:?})",
        t_total.elapsed(),
        t_http.elapsed()
    );
    out
}

async fn cmd_draft(input: &serde_json::Value) -> String {
    let t_total = std::time::Instant::now();
    eprintln!(
        "[gmail] cmd_draft to={:?} subject={:?}",
        input["to"].as_str(),
        input["subject"].as_str()
    );
    let raw = match build_raw_b64(input) {
        Ok(r) => r,
        Err(e) => return format!(r#"{{"error":{}}}"#, serde_json::json!(e)),
    };

    let token = match bearer().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e));
        }
    };

    let t_http = std::time::Instant::now();
    let resp: Result<reqwest::Response, reqwest::Error> = http()
        .post(format!("{GMAIL_BASE}/drafts"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"message": {"raw": raw}}))
        .send()
        .await;

    let out = match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            serde_json::to_string(&serde_json::json!({
                "status": "draft_saved",
                "id": body["id"].as_str().unwrap_or_default(),
            }))
            .unwrap_or_else(|_| r#"{"status":"draft_saved"}"#.to_string())
        }
        Ok(r) => {
            let status = r.status().as_u16();
            let text: String = r.text().await.unwrap_or_default();
            eprintln!("[integration:gmail] drafts.create HTTP {status}: {text}");
            format!(r#"{{"error":"HTTP {status}"}}"#)
        }
        Err(e) => {
            eprintln!("[integration:gmail] drafts.create failed: {e}");
            format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string()))
        }
    };
    eprintln!(
        "[gmail] cmd_draft TOTAL {:?} (http {:?})",
        t_total.elapsed(),
        t_http.elapsed()
    );
    out
}

async fn cmd_unread_count() -> String {
    let t_total = std::time::Instant::now();
    eprintln!("[gmail] cmd_unread_count");
    let token = match bearer().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e));
        }
    };

    let t_http = std::time::Instant::now();
    let resp: Result<reqwest::Response, reqwest::Error> = http()
        .get(format!("{GMAIL_BASE}/labels/INBOX"))
        .bearer_auth(&token)
        .send()
        .await;

    let out = match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            let count = body["messagesUnread"].as_i64().unwrap_or(0);
            format!(r#"{{"unread":{count}}}"#)
        }
        Ok(r) => {
            let status = r.status().as_u16();
            format!(r#"{{"error":"HTTP {status}"}}"#)
        }
        Err(e) => {
            eprintln!("[integration:gmail] labels.get INBOX failed: {e}");
            format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string()))
        }
    };
    eprintln!(
        "[gmail] cmd_unread_count TOTAL {:?} (http {:?})",
        t_total.elapsed(),
        t_http.elapsed()
    );
    out
}

async fn cmd_modify(input: &serde_json::Value, body: serde_json::Value) -> String {
    let t_total = std::time::Instant::now();
    let id = match input["id"].as_str() {
        Some(id) => id,
        None => return r#"{"error":"missing 'id' field"}"#.to_string(),
    };
    eprintln!("[gmail] cmd_modify id={id} body={body}");

    let token = match bearer().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return format!(r#"{{"error":{}}}"#, serde_json::json!(e));
        }
    };

    let t_http = std::time::Instant::now();
    let resp: Result<reqwest::Response, reqwest::Error> = http()
        .post(format!("{GMAIL_BASE}/messages/{id}/modify"))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await;

    let out = match resp {
        Ok(r) if r.status().is_success() => r#"{"status":"ok"}"#.to_string(),
        Ok(r) => {
            let status = r.status().as_u16();
            let text: String = r.text().await.unwrap_or_default();
            eprintln!("[integration:gmail] messages.modify HTTP {status}: {text}");
            format!(r#"{{"error":"HTTP {status}"}}"#)
        }
        Err(e) => {
            eprintln!("[integration:gmail] messages.modify failed for {id}: {e}");
            format!(r#"{{"error":{}}}"#, serde_json::json!(e.to_string()))
        }
    };
    eprintln!(
        "[gmail] cmd_modify TOTAL {:?} (http {:?})",
        t_total.elapsed(),
        t_http.elapsed()
    );
    out
}

fn parse_headers(msg: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(headers) = msg["payload"]["headers"].as_array() {
        for h in headers {
            if let (Some(name), Some(value)) = (h["name"].as_str(), h["value"].as_str()) {
                map.insert(name.to_lowercase(), value.to_string());
            }
        }
    }
    map
}

fn extract_body(payload: &serde_json::Value) -> String {
    if payload.is_null() {
        return String::new();
    }

    let mime = payload["mimeType"].as_str().unwrap_or("");

    if mime == "text/plain"
        && let Some(data) = payload["body"]["data"].as_str()
        && let Ok(bytes) = URL_SAFE_NO_PAD.decode(data.trim_end_matches('='))
    {
        return String::from_utf8_lossy(&bytes).into_owned();
    }

    if let Some(parts) = payload["parts"].as_array() {
        let plain = parts.iter().find(|p| p["mimeType"] == "text/plain");
        if let Some(p) = plain {
            let text = extract_body(p);
            if !text.is_empty() {
                return text;
            }
        }
        let html = parts.iter().find(|p| p["mimeType"] == "text/html");
        if let Some(p) = html {
            return strip_html(&extract_body(p));
        }
        for p in parts {
            let text = extract_body(p);
            if !text.is_empty() {
                return text;
            }
        }
    }

    String::new()
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn build_raw_b64(input: &serde_json::Value) -> Result<String, String> {
    let to = input["to"].as_str().ok_or("missing 'to' field")?;
    let subject = input["subject"].as_str().ok_or("missing 'subject' field")?;
    let body = input["body"].as_str().ok_or("missing 'body' field")?;
    let cc = input["cc"].as_str();

    let mut rfc = format!(
        "MIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\n\
         To: {to}\r\nSubject: {subject}\r\n"
    );
    if let Some(cc_addr) = cc {
        rfc.push_str(&format!("Cc: {cc_addr}\r\n"));
    }
    rfc.push_str("\r\n");
    rfc.push_str(body);

    Ok(URL_SAFE_NO_PAD.encode(rfc.as_bytes()))
}
