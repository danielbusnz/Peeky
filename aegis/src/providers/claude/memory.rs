//! Memory path. Used when the classifier returns Intent::Memory (the
//! user wants to store a fact about themselves like "remember my X is Y",
//! or recall one they told us before like "what did I tell you about Z").
//!
//! Storage: append-only JSONL at `~/.config/aegis/memory.jsonl`. Each
//! line is `{ "key": "...", "value": "...", "ts": "..." }`. The whole
//! file gets loaded into RAM at session start and refreshed whenever a
//! write happens. Effectively flat key-value with overwrite-on-rewrite
//! (latest entry wins per key when loading).
//!
//! Two operations:
//!   * STORE: user said "remember X". A short Claude call extracts a
//!     (key, value) pair from the transcript, we append to the JSONL,
//!     reply "Got it" via TTS.
//!   * RECALL: user said "what's my X" or "what did I tell you about Y".
//!     A short Claude call composes the answer using the loaded memory
//!     as context. Streams to TTS.
//!
//! The orchestrator decides which based on the transcript. Or we let
//! Claude decide via the same tool-call shape used in the classifier.

use super::Claude;
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Thread-safe wrapper around the in-memory map of user facts. Cloning
/// is cheap (Arc bumps a refcount). The orchestrator holds one of these
/// for the session and passes it into memory() / chat() / agent_loop().
#[derive(Clone)]
pub struct MemoryStore {
    inner: Arc<Mutex<MemoryInner>>,
}

struct MemoryInner {
    /// Latest value per key, keyed by lowercase key string.
    facts: Vec<(String, String)>,
    /// Path to the JSONL file backing this store.
    path: PathBuf,
}

impl MemoryStore {
    /// Open the store at the default location. Creates parent directory
    /// if it doesn't exist. Loads any existing facts. Errors only on
    /// filesystem issues. Missing file is treated as empty store.
    pub fn open_default() -> Result<Self, Box<dyn std::error::Error>> {
        let mut path = dirs::config_dir().ok_or("could not locate config dir")?;
        path.push("aegis");
        std::fs::create_dir_all(&path)?;
        path.push("memory.jsonl");
        Self::open(path)
    }

    /// Open the store at a specific path. Useful for tests.
    pub fn open(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let facts = load_facts(&path).unwrap_or_default();
        eprintln!(
            "[memory] loaded {} facts from {}",
            facts.len(),
            path.display()
        );
        Ok(Self {
            inner: Arc::new(Mutex::new(MemoryInner { facts, path })),
        })
    }

    /// Render the current set of facts as a multi-line string suitable
    /// for injection into a system prompt. Returns None if the store is
    /// empty so callers can skip the cache_control block entirely.
    pub fn as_prompt_block(&self) -> Option<String> {
        let inner = self.inner.lock().ok()?;
        if inner.facts.is_empty() {
            return None;
        }
        let mut s = String::new();
        for (k, v) in &inner.facts {
            s.push_str(&format!("- {}: {}\n", k, v));
        }
        Some(s)
    }

    /// Append a fact and update the in-memory view. Latest write wins
    /// on conflict at load time.
    pub fn store_fact(&self, key: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut inner = self.inner.lock().map_err(|_| "memory mutex poisoned")?;
        let key = key.trim().to_lowercase();
        let value = value.trim().to_string();
        let ts = chrono_like_now();
        let line = serde_json::json!({
            "key": key,
            "value": value,
            "ts": ts,
        })
        .to_string();
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&inner.path)?;
        writeln!(f, "{}", line)?;
        // Update in-memory view: replace existing key, else append.
        if let Some(slot) = inner.facts.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = value;
        } else {
            inner.facts.push((key, value));
        }
        Ok(())
    }
}

/// Read the JSONL store and build the latest-wins view. Missing file is
/// not an error (returns empty). Malformed lines are silently skipped so
/// a single corrupt entry can't kill the whole session.
fn load_facts(path: &PathBuf) -> std::io::Result<Vec<(String, String)>> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut map: Vec<(String, String)> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(key) = v["key"].as_str() else {
            continue;
        };
        let Some(value) = v["value"].as_str() else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = value.trim().to_string();
        if let Some(slot) = map.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = value;
        } else {
            map.push((key, value));
        }
    }
    Ok(map)
}

/// Crude RFC3339-ish timestamp without pulling chrono. Good enough for
/// audit / inspection of the JSONL file.
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch:{}", secs)
}

impl Claude {
    /// Handle an Intent::Memory turn. Routes to either store or recall
    /// based on a short Claude call that classifies the sub-intent and
    /// (for stores) extracts a (key, value) pair. Streams the spoken
    /// reply via `on_text_delta`.
    pub async fn memory<T>(
        &self,
        transcript: &str,
        store: &MemoryStore,
        mut on_text_delta: T,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        T: FnMut(&str),
    {
        // Ask Claude to either extract a fact (store) or formulate a
        // recall query. Single forced tool call so we get structured
        // output every time.
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 200,
            "stream": true,
            "system": [{
                "type": "text",
                "text": memory_router_prompt(),
                "cache_control": { "type": "ephemeral" }
            }],
            "tools": [
                {
                    "name": "store_fact",
                    "description": "User wants to remember a fact about themselves. \
                        Extract a short snake_case key and the literal value.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {
                                "type": "string",
                                "description": "snake_case identifier, e.g. 'favorite_color', 'allergic_to', 'home_city'"
                            },
                            "value": {
                                "type": "string",
                                "description": "the literal value the user provided"
                            }
                        },
                        "required": ["key", "value"]
                    }
                },
                {
                    "name": "recall_fact",
                    "description": "User wants to recall a previously-stored fact. \
                        Provide the snake_case key they're asking about (best guess based on phrasing).",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {
                                "type": "string",
                                "description": "snake_case key matching whatever 'remember X' might have stored"
                            }
                        },
                        "required": ["key"]
                    }
                }
            ],
            "tool_choice": { "type": "any" },
            "messages": [
                { "role": "user", "content": transcript }
            ]
        });

        let t_send = std::time::Instant::now();
        let response = self
            .http
            .post(&self.endpoint)
            .header(&self.auth.0, &self.auth.1)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(format!("memory router API error {}: {}", status, body_text).into());
        }

        // Parse the streamed tool call.
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut tool_name: Option<String> = None;
        let mut tool_json_buffer = String::new();
        let mut in_tool = false;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let s = std::str::from_utf8(&chunk)?;
            buffer.push_str(s);
            while let Some(idx) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..idx + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };
                    match event["type"].as_str() {
                        Some("content_block_start") => {
                            if event["content_block"]["type"].as_str() == Some("tool_use") {
                                if tool_name.is_none() {
                                    tool_name =
                                        event["content_block"]["name"].as_str().map(str::to_string);
                                }
                                in_tool = true;
                                tool_json_buffer.clear();
                            } else {
                                in_tool = false;
                            }
                        }
                        Some("content_block_delta") => {
                            if in_tool
                                && event["delta"]["type"].as_str() == Some("input_json_delta")
                                && let Some(j) = event["delta"]["partial_json"].as_str()
                            {
                                tool_json_buffer.push_str(j);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        eprintln!(
            "[memory] router tool={:?} → {:?}",
            tool_name,
            t_send.elapsed()
        );

        let tool_input: serde_json::Value = if tool_json_buffer.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&tool_json_buffer).unwrap_or(serde_json::json!({}))
        };

        match tool_name.as_deref() {
            Some("store_fact") => {
                let key = tool_input["key"].as_str().unwrap_or("").to_string();
                let value = tool_input["value"].as_str().unwrap_or("").to_string();
                if key.is_empty() || value.is_empty() {
                    let reply = "I couldn't tell what to remember. Try saying \
                                'remember X is Y' more directly.";
                    on_text_delta(reply);
                    return Ok(reply.to_string());
                }
                store.store_fact(&key, &value).map_err(
                    |e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() },
                )?;
                let reply = format!(
                    "Got it. I'll remember your {} is {}.",
                    key.replace('_', " "),
                    value
                );
                on_text_delta(&reply);
                Ok(reply)
            }
            Some("recall_fact") => {
                let key = tool_input["key"].as_str().unwrap_or("").to_string();
                let value = {
                    let inner = store.inner.lock().ok();
                    inner.and_then(|i| {
                        i.facts
                            .iter()
                            .find(|(k, _)| k == &key)
                            .map(|(_, v)| v.clone())
                    })
                };
                let reply = match value {
                    Some(v) => format!("Your {} is {}.", key.replace('_', " "), v),
                    None => format!(
                        "I don't have a {} on file. You can tell me by saying 'remember my {} is...'",
                        key.replace('_', " "),
                        key.replace('_', " ")
                    ),
                };
                on_text_delta(&reply);
                Ok(reply)
            }
            other => {
                let reply = format!(
                    "I wasn't sure what to do with that (router returned {:?}). Try \
                     'remember my X is Y' or 'what's my X'.",
                    other
                );
                on_text_delta(&reply);
                Ok(reply)
            }
        }
    }
}

/// Router prompt for the memory path. Stable, cached.
fn memory_router_prompt() -> &'static str {
    "You are the memory router for aegis, a desktop voice assistant. The \
user is either asking to remember a fact about themselves or asking to \
recall one they previously stored.\n\
\n\
Call EXACTLY ONE of:\n\
- store_fact(key, value): user said \"remember my X is Y\" or stated a \
fact about themselves directly. Extract a snake_case key + literal \
value.\n\
- recall_fact(key): user is asking \"what's my X\" or \"what did I tell \
you about X\". Provide the snake_case key you'd expect a prior store \
to have used.\n\
\n\
Examples:\n\
  \"remember my favorite color is blue\" → store_fact(favorite_color, blue)\n\
  \"remember I live in Boston\" → store_fact(home_city, Boston)\n\
  \"I'm allergic to peanuts\" → store_fact(allergic_to, peanuts)\n\
  \"what's my favorite color\" → recall_fact(favorite_color)\n\
  \"where do I live\" → recall_fact(home_city)\n\
  \"what am I allergic to\" → recall_fact(allergic_to)\n\
\n\
Always call a tool. Never respond with plain text."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_recall_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("aegis-mem-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let store = MemoryStore::open(tmp.clone()).expect("open");
        store.store_fact("name", "Dan").unwrap();
        store.store_fact("home_city", "Boston").unwrap();
        let prompt = store.as_prompt_block().unwrap();
        assert!(prompt.contains("name: Dan"));
        assert!(prompt.contains("home_city: Boston"));

        // Re-open and verify persistence.
        let store2 = MemoryStore::open(tmp.clone()).expect("reopen");
        let prompt2 = store2.as_prompt_block().unwrap();
        assert!(prompt2.contains("name: Dan"));
        assert!(prompt2.contains("home_city: Boston"));

        // Overwrite test.
        store2.store_fact("name", "Daniel").unwrap();
        let store3 = MemoryStore::open(tmp.clone()).expect("reopen 2");
        let prompt3 = store3.as_prompt_block().unwrap();
        assert!(prompt3.contains("name: Daniel"));
        assert!(!prompt3.contains("name: Dan\n"));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn empty_store_returns_none() {
        let tmp =
            std::env::temp_dir().join(format!("aegis-mem-empty-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let store = MemoryStore::open(tmp.clone()).expect("open");
        assert!(store.as_prompt_block().is_none());
        let _ = std::fs::remove_file(&tmp);
    }
}
