//! Local ONNX intent classifier. Replaces the Claude LLM fallback for the
//! hybrid classify path: keyword classifier first, then this if the keyword
//! classifier returns None. Runs entirely on-device, no network round-trip.
//!
//! Pipeline:
//!   text -> tokenizer (BERT WordPiece) -> [input_ids, attention_mask,
//!   token_type_ids] -> BERT embedder ONNX -> [1, 384] L2-normalized
//!   embedding -> logistic-regression head -> argmax -> Intent.
//!
//! The embedder is an fp32 ONNX model exported at opset 14 with LayerNorm
//! decomposed into primitive ops so tract can load it. The head is a small
//! JSON file: 5x384 coefficient matrix + 5 intercepts + 5 label strings.

use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;
use std::time::SystemTime;

use regex::Regex;
use tokenizers::Tokenizer;
use tract_onnx::prelude::*;

use crate::providers::claude::Intent;

/// Type alias for the optimized, runnable tract model.
type RunnableModel = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

/// Loaded, ready-to-run routelet classifier. Holds the ONNX model (already
/// optimized and compiled into a runnable plan), the tokenizer, and the
/// logistic-regression head weights.
pub struct Routelet {
    model: RunnableModel,
    tokenizer: Tokenizer,
    /// Coefficient matrix: coef[class][dim], shape [5, 384].
    coef: Vec<Vec<f32>>,
    /// Per-class intercept, length 5.
    intercept: Vec<f32>,
    /// Label string for each class, length 5. Maps argmax index to an intent
    /// string recognized by Intent::from_str.
    labels: Vec<String>,
    /// Temperature applied to logits before softmax. Values >1.0 flatten the
    /// distribution (lower confidence); <1.0 sharpen it. Loaded from head.json;
    /// defaults to 1.0 if the field is absent.
    temperature: f32,
}

impl Routelet {
    /// Load the classifier from `dir`. Expects three files:
    ///   embedder.onnx, tokenizer.json, head.json.
    ///
    /// On success returns a ready-to-use Routelet. Called once at startup;
    /// any error here is fatal (the caller should fail loud).
    pub fn load(dir: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let onnx_path = dir.join("embedder.onnx");
        let tok_path = dir.join("tokenizer.json");
        let head_path = dir.join("head.json");

        // Load and compile the ONNX embedder. The graph has three int64 inputs
        // [batch=1, seq] with a dynamic sequence dimension; tract resolves that
        // at run time from the shape of the tensors we pass in, so no explicit
        // input facts are needed here.
        let model = tract_onnx::onnx()
            .model_for_path(&onnx_path)
            .map_err(|e| format!("failed to load {}: {e}", onnx_path.display()))?
            .into_optimized()
            .map_err(|e| format!("failed to optimize {}: {e}", onnx_path.display()))?
            .into_runnable()
            .map_err(|e| {
                format!(
                    "failed to build runnable plan for {}: {e}",
                    onnx_path.display()
                )
            })?;

        // Load the HuggingFace tokenizer.json.
        let tokenizer = Tokenizer::from_file(&tok_path)
            .map_err(|e| format!("failed to load {}: {e}", tok_path.display()))?;

        // Parse head.json: { coef: [[384 f32] x 5], intercept: [5 f32], labels: [5 str] }
        let head_bytes = std::fs::read(&head_path)
            .map_err(|e| format!("failed to read {}: {e}", head_path.display()))?;
        let head: serde_json::Value = serde_json::from_slice(&head_bytes)
            .map_err(|e| format!("failed to parse {}: {e}", head_path.display()))?;

        let coef = head["coef"]
            .as_array()
            .ok_or("head.json: 'coef' is not an array")?
            .iter()
            .enumerate()
            .map(|(i, row)| {
                row.as_array()
                    .ok_or_else(|| format!("head.json: coef[{i}] is not an array"))?
                    .iter()
                    .enumerate()
                    .map(|(j, v)| {
                        v.as_f64()
                            .ok_or_else(|| format!("head.json: coef[{i}][{j}] is not a number"))
                            .map(|f| f as f32)
                    })
                    .collect::<Result<Vec<f32>, String>>()
            })
            .collect::<Result<Vec<Vec<f32>>, String>>()?;

        let intercept = head["intercept"]
            .as_array()
            .ok_or("head.json: 'intercept' is not an array")?
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.as_f64()
                    .ok_or_else(|| format!("head.json: intercept[{i}] is not a number"))
                    .map(|f| f as f32)
            })
            .collect::<Result<Vec<f32>, String>>()?;

        let labels = head["labels"]
            .as_array()
            .ok_or("head.json: 'labels' is not an array")?
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.as_str()
                    .ok_or_else(|| format!("head.json: labels[{i}] is not a string"))
                    .map(str::to_owned)
            })
            .collect::<Result<Vec<String>, String>>()?;

        // Optional temperature field; default to 1.0 (no scaling) if absent.
        let temperature = head["temperature"]
            .as_f64()
            .map(|v| v as f32)
            .unwrap_or(1.0);

        if coef.len() != labels.len() || intercept.len() != labels.len() {
            return Err(format!(
                "head.json shape mismatch: {} classes in coef, {} in intercept, {} labels",
                coef.len(),
                intercept.len(),
                labels.len()
            )
            .into());
        }

        Ok(Routelet {
            model,
            tokenizer,
            coef,
            intercept,
            labels,
            temperature,
        })
    }

    /// Classify `text` and return the predicted intent together with a
    /// calibrated confidence in `[0.0, 1.0]` (the softmax probability of the
    /// winning class after temperature scaling).
    ///
    /// Returns `None` if inference fails or the argmax label is unrecognized.
    /// Synchronous CPU inference; target latency under 30ms on a modern desktop.
    pub fn classify_with_confidence(&self, text: &str) -> Option<(Intent, f32)> {
        let normalized = preprocess(text);
        let embedding = self.embed(&normalized).ok()?;
        head_predict_with_confidence(
            &self.coef,
            &self.intercept,
            self.temperature,
            &self.labels,
            &embedding,
        )
    }

    /// Classify `text` into one of the five intents. Returns None if the
    /// model runs successfully but the predicted label is unrecognized (which
    /// should not happen with a correctly exported head).
    ///
    /// Thin wrapper around `classify_with_confidence`; drops the confidence
    /// score. Synchronous CPU inference; target latency is under 30ms on a
    /// modern desktop. Does not require a tokio runtime.
    pub fn classify(&self, text: &str) -> Option<Intent> {
        self.classify_with_confidence(text)
            .map(|(intent, _)| intent)
    }

    /// Run the BERT embedder and return the [384] embedding vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
        // Tokenize. add_special_tokens=true prepends [CLS] and appends [SEP].
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("tokenizer encode failed: {e}"))?;

        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&x| x as i64)
            .collect();
        let type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&x| x as i64).collect();
        let seq = ids.len();

        // Build [1, seq] int64 tensors for each of the three inputs.
        let input_ids: Tensor = tract_ndarray::Array2::from_shape_vec((1, seq), ids)?.into();
        let attention_mask: Tensor = tract_ndarray::Array2::from_shape_vec((1, seq), mask)?.into();
        let token_type_ids: Tensor =
            tract_ndarray::Array2::from_shape_vec((1, seq), type_ids)?.into();

        let outputs = self.model.run(tvec!(
            input_ids.into(),
            attention_mask.into(),
            token_type_ids.into()
        ))?;

        // Output is "embedding", float32 [1, 384]. Flatten to [384].
        let view = outputs[0].to_array_view::<f32>()?;
        let embedding: Vec<f32> = view.iter().copied().collect();
        Ok(embedding)
    }
}

/// Pure head computation: dot-product logits, temperature scaling, numerically
/// stable softmax, argmax, label lookup. Separated from `embed` so the math
/// can be exercised in hermetic unit tests without loading the ONNX model.
///
/// Returns `None` when `coef`/`intercept`/`labels` are empty, no argmax
/// winner exists (all NaN), or the winning label is not a recognized intent.
fn head_predict_with_confidence(
    coef: &[Vec<f32>],
    intercept: &[f32],
    temperature: f32,
    labels: &[String],
    embedding: &[f32],
) -> Option<(Intent, f32)> {
    let num_classes = labels.len();
    if num_classes == 0 {
        return None;
    }

    let mut logits: Vec<f32> = Vec::with_capacity(num_classes);
    for c in 0..num_classes {
        let dot: f32 = coef[c]
            .iter()
            .zip(embedding.iter())
            .map(|(a, b)| a * b)
            .sum();
        logits.push(dot + intercept[c]);
    }

    // Temperature scaling: divide logits before softmax.
    // temperature=1.0 is identity; values != 1.0 were stored in head.json.
    let temp = if temperature > 0.0 { temperature } else { 1.0 };
    for v in &mut logits {
        *v /= temp;
    }

    // Numerically stable softmax: subtract max before exp.
    let max_logit = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut exps: Vec<f32> = logits.iter().map(|&v| (v - max_logit).exp()).collect();
    let sum: f32 = exps.iter().sum();
    for v in &mut exps {
        *v /= sum;
    }

    // Argmax of the softmax probabilities (same ordering as raw logits).
    let (best_class, &best_prob) = exps
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))?;

    let intent = Intent::from_str(labels[best_class].as_str())?;
    Some((intent, best_prob))
}

// ────── redaction ──────

struct Redactors {
    /// Matches assignment-cue phrases and captures the value that follows.
    /// Used only for Memory intent to mask stored values like "my name is Daniel".
    memory_assign: Regex,
    /// After a secret keyword (password, token, etc.), masks the remainder of
    /// the phrase to prevent credential leakage in any intent.
    secret_keyword: Regex,
    /// Email addresses.
    email: Regex,
    /// Runs of 4 or more digits (card numbers, PINs, phone numbers, etc.).
    /// Short numbers like "50" or "3" are intentionally left alone.
    digit_run: Regex,
}

static REDACTORS: OnceLock<Redactors> = OnceLock::new();

fn redactors() -> &'static Redactors {
    REDACTORS.get_or_init(|| Redactors {
        // reason: every pattern is a static literal. A malformed one would fail
        // deterministically on first use and the unit tests would catch it, so
        // these unwraps cannot fire at runtime.
        // Keep everything up to and including the cue word; replace trailing value.
        memory_assign: Regex::new(r"(?i)\b(is|are|=|equals)\b\s+\S.*$").unwrap(),
        // Keep the keyword itself; blank out everything that follows.
        secret_keyword: Regex::new(
            r"(?i)\b(password|passcode|pin|ssn|secret|token|api\s*key|api\s*secret|credit card|card number)\b.*$",
        )
        .unwrap(),
        email: Regex::new(r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}").unwrap(),
        digit_run: Regex::new(r"\b\d{4,}\b").unwrap(),
    })
}

/// Intent-independent normalization applied identically at training and
/// inference. Masks secrets, emails, and long digit runs. Does NOT apply the
/// memory assign-cue rule, which is intent-dependent and storage-only.
///
/// Rules in order (earlier rules can consume text that later rules never see):
///   1. Secret keyword tail: keep the keyword, mask everything after it.
///   2. Email addresses -> `<EMAIL>`.
///   3. Runs of 4+ digits -> `<NUM>`.
fn preprocess(text: &str) -> String {
    let r = redactors();

    // Rule 1: "password is hunter2" -> "password <SECRET>".
    let s = r
        .secret_keyword
        .replace(text, |caps: &regex::Captures| {
            format!("{} <SECRET>", &caps[1])
        })
        .into_owned();

    // Rule 2: emails.
    let s = r.email.replace_all(&s, "<EMAIL>").into_owned();

    // Rule 3: runs of 4+ digits (PINs, card numbers, phone numbers, etc.).
    r.digit_run.replace_all(&s, "<NUM>").into_owned()
}

/// Redact PII and secrets from a voice transcript before writing to the log.
/// Applies `preprocess` for all intents, then the memory assign-cue rule for
/// Memory turns. Over-redaction is acceptable; under-redaction is not.
fn redact(text: &str, intent: Intent) -> String {
    let s = preprocess(text);

    // Memory assign-cue (Memory intent only). "my name is Daniel" ->
    // "my name is <SECRET>". Applied after preprocess so credentials already
    // masked there are not re-processed.
    if intent == Intent::Memory {
        redactors()
            .memory_assign
            .replace(&s, |caps: &regex::Captures| {
                format!("{} <SECRET>", &caps[1])
            })
            .into_owned()
    } else {
        s
    }
}

/// Retention cap for the opt-in log: keep at most this many of the most recent
/// lines on disk so redacted history can't grow without bound.
const LOG_MAX_LINES: usize = 5000;

/// Append a single line to `path` (creating it if needed), then trim the file
/// to its most recent `max_lines` lines. Returns an IO error so the caller can
/// log and swallow it. The log is low-volume and capped, so reading it back per
/// write is cheap, and the file is only rewritten once it grows past the cap.
fn append_record(path: &Path, line: &str, max_lines: usize) -> std::io::Result<()> {
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{line}")?;
    }
    let contents = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = contents.lines().collect();
    if lines.len() > max_lines {
        let tail = lines[lines.len() - max_lines..].join("\n");
        std::fs::write(path, format!("{tail}\n"))?;
    }
    Ok(())
}

/// Opt-in, redacted on-device log for Stage 1 distillation data collection.
/// Does nothing unless `AEGIS_ROUTELET_LOG=1`. On any error, emits a warning
/// to stderr and returns without panicking. Safe to call in the hot turn path.
pub fn log_classification(text: &str, intent: Intent) {
    if std::env::var("AEGIS_ROUTELET_LOG").as_deref() != Ok("1") {
        return;
    }

    let redacted = redact(text, intent);
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let record = match serde_json::to_string(&serde_json::json!({
        "redacted_text": redacted,
        "predicted": intent.as_str(),
        "ts": ts,
    })) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[routelet-log] serialization error: {e}");
            return;
        }
    };

    let log_path = match build_log_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[routelet-log] could not resolve log path: {e}");
            return;
        }
    };

    if let Err(e) = append_record(&log_path, &record, LOG_MAX_LINES) {
        eprintln!("[routelet-log] write error ({}): {e}", log_path.display());
    }
}

/// Resolve `~/.config/aegis/routelet_log.jsonl`, creating the directory if
/// needed. Mirrors the resolution used by `MemoryStore::open_default`.
fn build_log_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let mut path = dirs::config_dir().ok_or("could not locate config dir")?;
    path.push("aegis");
    std::fs::create_dir_all(&path)?;
    path.push("routelet_log.jsonl");
    Ok(path)
}

// ────── tests ──────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    fn load_test_routelet() -> Option<Routelet> {
        // Models live at models/routelet relative to the repo root; cargo test
        // runs with cwd = the crate root (aegis/), so step up one level.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("models/routelet");
        Routelet::load(&dir).ok()
    }

    // confidence: value is always in [0, 1] and clear phrases score above 0.5.
    #[test]
    fn confidence_range_and_clear_phrase() {
        let Some(routelet) = load_test_routelet() else {
            eprintln!("[test] skipping: model not found");
            return;
        };

        let phrases = [
            "play despacito on spotify",
            "what is the capital of france",
            "where is the search bar",
            "remember my birthday is in june",
            "open youtube and play the top result",
        ];

        for phrase in &phrases {
            let (_, conf) = routelet
                .classify_with_confidence(phrase)
                .unwrap_or_else(|| panic!("classify_with_confidence returned None for: {phrase}"));
            assert!(
                (0.0..=1.0).contains(&conf),
                "confidence {conf} out of [0,1] for: {phrase}"
            );
        }

        // The canonical integration phrase must map to Intent::Integration with
        // confidence well above chance (> 0.5).
        let (intent, conf) = routelet
            .classify_with_confidence("play despacito on spotify")
            .expect("must classify");
        assert_eq!(
            intent,
            Intent::Integration,
            "expected Integration for 'play despacito on spotify'"
        );
        assert!(
            conf > 0.5,
            "expected confidence > 0.5 for clear phrase, got {conf}"
        );
    }

    // redact: benign commands are untouched
    #[test]
    fn redact_benign_integration_unchanged() {
        let out = redact("play despacito on spotify", Intent::Integration);
        assert_eq!(out, "play despacito on spotify");
    }

    #[test]
    fn redact_benign_chat_unchanged() {
        let out = redact("what's the capital of france", Intent::Chat);
        assert_eq!(out, "what's the capital of france");
    }

    // redact: secret keyword masks trailing value
    #[test]
    fn redact_wifi_password() {
        let out = redact("my wifi password is hunter2", Intent::Memory);
        assert!(out.contains("<SECRET>"), "expected <SECRET> in: {out}");
        assert!(
            !out.contains("hunter2"),
            "hunter2 should be masked in: {out}"
        );
    }

    // redact: memory assign-cue masks stored value
    #[test]
    fn redact_remember_name() {
        let out = redact("remember my name is daniel", Intent::Memory);
        assert!(out.contains("<SECRET>"), "expected <SECRET> in: {out}");
        assert!(
            !out.contains("daniel"),
            "name value should be masked in: {out}"
        );
    }

    // redact: assign-cue does NOT fire for non-Memory intents
    #[test]
    fn redact_assign_cue_chat_not_masked() {
        // "the sky is blue" in a Chat turn should keep "blue"
        let out = redact("the sky is blue", Intent::Chat);
        assert!(
            out.contains("blue"),
            "non-memory assign cue should be left alone: {out}"
        );
    }

    // redact: email masking
    #[test]
    fn redact_email() {
        let out = redact("email me at a@b.com", Intent::Chat);
        assert!(out.contains("<EMAIL>"), "expected <EMAIL> in: {out}");
        assert!(!out.contains("a@b.com"), "email should be masked in: {out}");
    }

    // redact: 4+ digit runs masked
    #[test]
    fn redact_long_digit_run_memory() {
        let out = redact("my code is 904112", Intent::Memory);
        // The memory assign-cue rule fires first and masks "904112" as <SECRET>;
        // the digit rule does not need to fire for the value to be gone.
        assert!(
            !out.contains("904112"),
            "6-digit code should be masked in: {out}"
        );
        assert!(
            out.contains("<SECRET>") || out.contains("<NUM>"),
            "expected a mask token in: {out}"
        );
    }

    #[test]
    fn redact_long_digit_run_integration() {
        let out = redact("call 5551234", Intent::Integration);
        assert!(
            !out.contains("5551234"),
            "phone number should be masked in: {out}"
        );
        assert!(out.contains("<NUM>"), "expected <NUM> in: {out}");
    }

    // redact: small numbers (1-3 digits) are preserved
    #[test]
    fn redact_small_number_preserved() {
        let out = redact("set volume to 50", Intent::Integration);
        assert!(out.contains("50"), "two-digit number should survive: {out}");
    }

    // append_record: creates file, writes a valid JSON line, second call appends
    #[test]
    fn append_record_creates_and_appends() {
        // Use a unique name in tmp so parallel test runs don't collide.
        let path = std::env::temp_dir().join(format!(
            "aegis_routelet_test_{}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        // Remove any leftover from a previous run.
        let _ = std::fs::remove_file(&path);

        let line1 = r#"{"a":1}"#;
        let line2 = r#"{"b":2}"#;

        append_record(&path, line1, 10).expect("first write should succeed");
        append_record(&path, line2, 10).expect("second write should succeed");

        let file = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = std::io::BufReader::new(file)
            .lines()
            .map(|l| l.unwrap())
            .collect();

        // Clean up before assertions so a failure still removes the file.
        let _ = std::fs::remove_file(&path);

        assert_eq!(lines.len(), 2, "expected 2 lines");
        assert_eq!(lines[0], line1);
        assert_eq!(lines[1], line2);

        // Both lines must parse as valid JSON.
        serde_json::from_str::<serde_json::Value>(&lines[0]).expect("line 1 must be valid JSON");
        serde_json::from_str::<serde_json::Value>(&lines[1]).expect("line 2 must be valid JSON");
    }

    // append_record: trims the file to the most recent max_lines.
    #[test]
    fn append_record_caps_to_max_lines() {
        let path = std::env::temp_dir().join(format!(
            "aegis_routelet_cap_{}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);

        // Write 10 lines with a cap of 3; only the last 3 should remain.
        for i in 0..10 {
            append_record(&path, &format!(r#"{{"n":{i}}}"#), 3).expect("write should succeed");
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3, "file should be capped to 3 lines");
        assert_eq!(lines, vec![r#"{"n":7}"#, r#"{"n":8}"#, r#"{"n":9}"#]);
    }

    // preprocess: shared fixture conformance. Every vector in
    // tests/preprocess_vectors.json must match exactly.
    #[test]
    fn preprocess_conformance() {
        let raw = include_str!("../tests/preprocess_vectors.json");
        let fixture: serde_json::Value =
            serde_json::from_str(raw).expect("preprocess_vectors.json must parse");
        let vectors = fixture["vectors"]
            .as_array()
            .expect("'vectors' must be an array");
        for v in vectors {
            let input = v["in"].as_str().expect("vector 'in' must be a string");
            let want = v["out"].as_str().expect("vector 'out' must be a string");
            let got = preprocess(input);
            assert_eq!(got, want, "preprocess({input:?}) = {got:?}, want {want:?}");
        }
    }

    // Intent::as_str round-trips with from_str for all five variants.
    // (Mirrors the test in classifier.rs but lives here with the log code
    // so this module's dependency on as_str is tested directly.)
    #[test]
    fn as_str_round_trip_all_variants() {
        for intent in [
            Intent::FindAction,
            Intent::Integration,
            Intent::Chat,
            Intent::Memory,
            Intent::Agent,
        ] {
            assert_eq!(
                Intent::from_str(intent.as_str()),
                Some(intent),
                "round-trip failed for {:?}",
                intent
            );
        }
    }

    // ── hermetic head-math tests (no model, no file I/O, no network) ──

    /// Build the minimal inputs for head_predict_with_confidence.
    /// dim=3, 5 classes matching the canonical label order.
    fn make_labels() -> Vec<String> {
        ["agent", "chat", "find_action", "integration", "memory"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    // Argmax + label mapping: coef constructed so "chat" (index 1) wins clearly.
    // embedding = [1, 0, 0]; coef[1] = [10, 0, 0] gives logit 10; all others 0.
    #[test]
    fn head_argmax_picks_clear_winner() {
        let labels = make_labels();
        let dim = 3usize;
        let mut coef: Vec<Vec<f32>> = vec![vec![0.0; dim]; labels.len()];
        // Class 1 = "chat" wins with a large positive dot product.
        coef[1][0] = 10.0;
        let intercept = vec![0.0f32; labels.len()];
        let embedding = vec![1.0f32, 0.0, 0.0];

        let result = head_predict_with_confidence(&coef, &intercept, 1.0, &labels, &embedding);
        let (intent, conf) = result.expect("must return Some for a clear winner");
        assert_eq!(intent, Intent::Chat, "expected Chat to win");
        assert!(
            conf > 0.0 && conf <= 1.0,
            "confidence must be in (0, 1], got {conf}"
        );
    }

    // Confidence equals the max softmax probability, verified by hand for a
    // small 3-class case reduced from the 5-class setup.
    //
    // Classes: ["agent"=0, "chat"=1, "find_action"=2, "integration"=3, "memory"=4]
    // embedding=[1,0,0], coef row dot products: [0, 2, 0, 0, 0], intercepts all 0.
    // Logits (temp=1): [0, 2, 0, 0, 0].
    // Stable softmax: max=2, shifted=[−2, 0, −2, −2, −2],
    //   exps=[e^−2, 1, e^−2, e^−2, e^−2], sum = 1 + 4*e^−2.
    //   p_chat = 1 / (1 + 4*e^−2).
    #[test]
    fn head_confidence_matches_softmax_by_hand() {
        let labels = make_labels();
        let dim = 3usize;
        let mut coef: Vec<Vec<f32>> = vec![vec![0.0; dim]; labels.len()];
        coef[1][0] = 2.0; // "chat" gets logit 2, all others get 0.
        let intercept = vec![0.0f32; labels.len()];
        let embedding = vec![1.0f32, 0.0, 0.0];

        let (_, conf) = head_predict_with_confidence(&coef, &intercept, 1.0, &labels, &embedding)
            .expect("must return Some");

        let e_neg2 = (-2.0f32).exp();
        let expected = 1.0 / (1.0 + 4.0 * e_neg2);
        assert!(
            (conf - expected).abs() < 1e-5,
            "confidence {conf} differs from expected {expected} by more than 1e-5"
        );
    }

    // Temperature flattening: with higher temperature the winning probability
    // is strictly lower, but the argmax intent is unchanged.
    #[test]
    fn head_temperature_flattens_confidence() {
        let labels = make_labels();
        let dim = 3usize;
        let mut coef: Vec<Vec<f32>> = vec![vec![0.0; dim]; labels.len()];
        coef[3][0] = 5.0; // "integration" wins with logit 5.
        let intercept = vec![0.0f32; labels.len()];
        let embedding = vec![1.0f32, 0.0, 0.0];

        let (intent_t1, conf_t1) =
            head_predict_with_confidence(&coef, &intercept, 1.0, &labels, &embedding)
                .expect("must return Some at temperature 1.0");
        let (intent_t2, conf_t2) =
            head_predict_with_confidence(&coef, &intercept, 2.0, &labels, &embedding)
                .expect("must return Some at temperature 2.0");

        assert_eq!(
            intent_t1, intent_t2,
            "argmax must be the same at both temperatures"
        );
        assert_eq!(
            intent_t1,
            Intent::Integration,
            "expected Integration to win"
        );
        assert!(
            conf_t2 < conf_t1,
            "higher temperature must flatten confidence: t=2 gave {conf_t2}, t=1 gave {conf_t1}"
        );
    }

    // Unknown label: when the argmax winner has a label not recognized by
    // Intent::from_str, head_predict_with_confidence returns None.
    #[test]
    fn head_unknown_label_returns_none() {
        let mut labels = make_labels();
        // Replace the "chat" entry (index 1) with a bogus label so it becomes the
        // argmax winner but cannot be mapped to an Intent.
        labels[1] = "bogus_label".to_string();

        let dim = 3usize;
        let mut coef: Vec<Vec<f32>> = vec![vec![0.0; dim]; labels.len()];
        coef[1][0] = 10.0; // bogus_label wins by a large margin.
        let intercept = vec![0.0f32; labels.len()];
        let embedding = vec![1.0f32, 0.0, 0.0];

        let result = head_predict_with_confidence(&coef, &intercept, 1.0, &labels, &embedding);
        assert!(
            result.is_none(),
            "expected None when argmax label is unrecognized, got {result:?}"
        );
    }
}
