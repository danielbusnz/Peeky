//! Anthropic Claude provider. Public surface is the `Claude` client and the
//! `Action` enum it surfaces during the agent loop. The actual streaming
//! loop, tool parsing, and system prompt live in submodules.

mod agent_loop;
mod chat;
mod classifier;
mod find_action;
mod history;
mod integration;
mod memory;
mod parsing;
mod prompt;

// Re-exports for the aegis binary's use. Examples that pull in providers/
// via #[path] but don't touch Intent or MemoryStore see these as unused;
// the allow silences that false-positive warning without affecting the
// real aegis binary, which uses both.
#[allow(unused_imports)]
pub use classifier::Intent;
#[allow(unused_imports)]
pub use history::HistoryStore;
#[allow(unused_imports)]
pub use memory::MemoryStore;

/// A side-effecting action Claude requested via one of the tools in
/// `run_agent_loop`. The streaming parser surfaces these in real time so the
/// caller can fire them before the response is finished.
#[derive(Debug, Clone)]
pub enum Action {
    /// `computer` tool, `mouse_move`. Visual overlay moves to (x,y) but
    /// no real input is injected. Used for "where is X" / "show me X".
    Point { x: i64, y: i64 },
    /// `computer` tool, `left_click`. Visual overlay AND system mouse
    /// click at (x,y). Used for "click X" / "press X" / "select X".
    Click { x: i64, y: i64 },
    /// `computer` tool, `type`. Types `text` into the currently focused
    /// field. Used for "type X" / "search for X" / "write X". Embed a
    /// trailing \n if the result should be submitted (Enter).
    Type { text: String },
    /// `computer` tool, `key`. Press a key or key combination like
    /// "Return", "Tab", "Escape", "ctrl+a", "ctrl+f". The `key` string
    /// is whatever Claude emitted; parsing happens in the action handler.
    Key { key: String },
    /// `computer` tool, `scroll`. Direction is "up"/"down"/"left"/"right";
    /// amount is the number of "wheel clicks" Claude wants. Coordinate
    /// (if Claude provided one) is currently ignored. Wayland doesn't
    /// expose a clean "scroll at point" primitive without raw evdev.
    Scroll { direction: String, amount: u32 },
    /// `open_url` custom tool. URL is whatever Claude emitted; validation
    /// happens at execution time, not here.
    OpenUrl { url: String },
    /// `launch_app` custom tool. App is a desktop-file basename or a
    /// runnable binary name.
    LaunchApp { app: String },
    /// `switch_to_window` custom tool. Target is a window class or title.
    SwitchToWindow { target: String },
    /// An integration tool call (Spotify, etc.) we don't have a dedicated
    /// variant for. Dispatched at runtime via the integrations registry;
    /// the name + raw JSON payload are pulled from the outer SSE event, not
    /// from this variant. The variant exists purely so the on_action
    /// callback can tell integration tools apart from cursor-visible ones.
    Integration,
}

#[derive(Clone)]
pub struct Claude {
    pub http: reqwest::Client,
    /// Full URL to POST messages requests to. Either the hosted proxy or
    /// api.anthropic.com depending on which mode we're in.
    pub endpoint: String,
    /// (header_name, header_value) for auth. Either ("x-aegis-device-id", uuid)
    /// when routed through the proxy, or ("x-api-key", anthropic_key) in
    /// direct mode.
    pub auth: (String, String),
    /// True when this Claude is routed through the proxy. Controls whether
    /// we look for an invite code on each request. Direct mode skips the
    /// lookup since Anthropic ignores unknown headers and there's no point
    /// in the file read.
    pub via_proxy: bool,
}

/// Default endpoint for the hosted proxy. Override at compile time by setting
/// `AEGIS_PROXY_URL` to a different worker URL if you deploy your own.
const PROXY_URL: &str = "https://aegis-proxy.danielbusnz.workers.dev/v1/anthropic/messages";
const DIRECT_URL: &str = "https://api.anthropic.com/v1/messages";

impl Claude {
    /// Adds the auth header (always) and the invite code header (when set
    /// and in proxy mode). Reads the invite code from disk on every call so
    /// onboarding-time changes take effect on the very next request without
    /// restarting aegis. File read is ~100us cold and free hot; cheap given
    /// it's hit a handful of times per voice turn.
    pub fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = req.header(&self.auth.0, &self.auth.1);
        if !self.via_proxy {
            return req;
        }
        match super::invite_code::load() {
            Some(code) => req.header(crate::providers::proxy_contract::INVITE_CODE_HEADER, code),
            None => req,
        }
    }

    /// Open the HTTPS connection to our endpoint so the first real voice
    /// turn doesn't pay TLS handshake cost. Fires a deliberately-malformed
    /// request that fast-fails on the server; the TCP+TLS handshake leaves
    /// a warm connection in reqwest's pool. Response is discarded.
    pub async fn warm(&self) {
        let _ = self
            .apply_auth(self.http.post(&self.endpoint))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await;
    }

    /// Initialize from `.env`/environment. Default behavior is to route through
    /// the hosted aegis-proxy on Cloudflare, identified by a per-install UUID.
    /// No API key needed; that's the whole plug-and-play story.
    ///
    /// To bypass the proxy and talk to Anthropic directly (useful for local
    /// dev, debugging, or burning your own credit), set
    /// `AEGIS_ANTHROPIC_DIRECT=1` in the environment AND provide
    /// `ANTHROPIC_API_KEY`.
    ///
    /// `http` is the shared `reqwest::Client` so connection pools (TCP/TLS)
    /// are reused across calls. Saves the ~150ms handshake on every call
    /// after the first.
    pub fn from_env(http: reqwest::Client) -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();

        if std::env::var("AEGIS_ANTHROPIC_DIRECT").is_ok() {
            let api_key = std::env::var("ANTHROPIC_API_KEY")?;
            return Ok(Claude {
                http,
                endpoint: DIRECT_URL.to_string(),
                auth: ("x-api-key".to_string(), api_key),
                via_proxy: false,
            });
        }

        let device_id = super::device_id::load_or_create()?;
        Ok(Claude {
            http,
            endpoint: PROXY_URL.to_string(),
            auth: (
                crate::providers::proxy_contract::DEVICE_ID_HEADER.to_string(),
                device_id,
            ),
            via_proxy: true,
        })
    }
}
