//! Local ONNX intent classifier. Replaces the Claude LLM fallback for the
//! hybrid classify path: keyword classifier first, then this if the keyword
//! classifier returns None. Runs entirely on-device, no network round-trip.
//!
//! Pipeline:
//!   text -> tokenizer (BERT WordPiece) -> [input_ids, attention_mask,
//!   token_type_ids] -> BERT embedder ONNX -> [1, 384] L2-normalized
//!   embedding -> logistic-regression head -> argmax -> Intent.
//!
//! The embedder is an ONNX model exported at opset 14 with LayerNorm
//! decomposed into primitive ops so tract can load it. Weights are
//! dynamically quantized to int8 to keep the shipped artifact ~34MB. The
//! head is a small JSON file: 5x384 coefficient matrix + 5 intercepts + 5
//! label strings.
//!
//! Two sibling concerns live alongside the classifier: `redact` masks PII and
//! secrets out of transcripts, and `sample` collects redacted classification
//! samples for offline distillation (local log plus opt-in proxy upload).

mod redact;
mod sample;

use std::path::Path;

use tokenizers::Tokenizer;
use tract_onnx::prelude::*;

use crate::providers::claude::Intent;
use redact::preprocess;

// The distillation data path is public API used by the orchestrator and the
// session builder; keep the existing `crate::routelet::*` call sites working.
pub use sample::{init_uploader, log_classification};

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

#[cfg(test)]
mod tests {
    use super::*;

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
