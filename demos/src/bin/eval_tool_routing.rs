// Eval harness for top-level intent classification.
//
// Feeds each transcript from `peeky/evals/cases/tool_routing.json` through
// the live Claude classifier (`claude.classify_intent`) and compares the
// picked Intent to the case's `expected_intent`. The model never sees
// `expected_intent`; we only use it to score the result.
//
// Usage:
//   cargo run --release --bin eval_tool_routing
//   cargo run --release --bin eval_tool_routing -- peeky/evals/cases/tool_routing.json
//
// Requires the same .env setup as the main binary (proxy device id or
// PEEKY_ANTHROPIC_DIRECT=1 + ANTHROPIC_API_KEY).
//
// Prints a per-case pass/fail line, a per-category summary, and a
// confusion matrix. Each case is one Claude call, billed normally.

#![allow(dead_code)]

use peeky::providers;

use providers::claude::{Claude, Intent};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const DEFAULT_CASES_PATH: &str = "peeky/evals/cases/tool_routing.json";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cases_path = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or(DEFAULT_CASES_PATH);

    let cases = load_cases(cases_path);
    if cases.is_empty() {
        eprintln!("no cases found in {cases_path}");
        std::process::exit(1);
    }

    let http = reqwest::Client::new();
    let claude = Claude::from_env(http).expect("Claude init failed (check .env)");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    // Warm the HTTP pool so case 1 doesn't pay TLS handshake cost.
    rt.block_on(claude.warm());

    println!();
    println!("─── eval: tool routing ───");
    println!("cases file: {}", cases_path);
    println!("cases:      {}", cases.len());
    println!();
    println!(
        "{:<3} {:<24} {:<13} {:<13} {:>7}  transcript",
        "", "id", "expected", "actual", "ms"
    );

    let mut results: Vec<RunResult> = Vec::with_capacity(cases.len());
    for case in &cases {
        let t = Instant::now();
        let intent = rt
            .block_on(claude.classify_intent(&case.transcript))
            .ok()
            .flatten();
        let dur = t.elapsed();
        let actual = intent_label(intent);
        let pass = actual == case.expected_intent;
        let mark = if pass { "✓" } else { "✗" };
        println!(
            "{:<3} {:<24} {:<13} {:<13} {:>5}ms  {}",
            mark,
            case.id,
            case.expected_intent,
            actual,
            dur.as_millis(),
            case.transcript
        );
        results.push(RunResult {
            case: case.clone(),
            actual_intent: intent,
            duration: dur,
            pass,
        });
    }

    print_summary(&results);
}

#[derive(Clone)]
struct Case {
    id: String,
    transcript: String,
    expected_intent: String,
    category: String,
}

struct RunResult {
    case: Case,
    actual_intent: Option<Intent>,
    duration: Duration,
    pass: bool,
}

fn load_cases(path: &str) -> Vec<Case> {
    let raw =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("could not read {path}: {e}"));
    let json: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("could not parse {path}: {e}"));
    let arr = json["cases"]
        .as_array()
        .unwrap_or_else(|| panic!("{path} has no `cases` array"));
    arr.iter()
        .map(|v| Case {
            id: v["id"].as_str().unwrap_or("").to_string(),
            transcript: v["transcript"].as_str().unwrap_or("").to_string(),
            expected_intent: v["expected_intent"].as_str().unwrap_or("").to_string(),
            category: v["category"]
                .as_str()
                .unwrap_or("uncategorized")
                .to_string(),
        })
        .collect()
}

fn intent_label(i: Option<Intent>) -> String {
    match i {
        Some(Intent::FindAction) => "FindAction".to_string(),
        Some(Intent::Integration) => "Integration".to_string(),
        Some(Intent::Chat) => "Chat".to_string(),
        Some(Intent::Memory) => "Memory".to_string(),
        Some(Intent::Agent) => "Agent".to_string(),
        None => "(none)".to_string(),
    }
}

fn print_summary(results: &[RunResult]) {
    let total = results.len();
    let passed = results.iter().filter(|r| r.pass).count();
    let pct = if total == 0 {
        0.0
    } else {
        100.0 * passed as f64 / total as f64
    };

    println!();
    println!("─── summary ───");
    println!("overall: {}/{} ({:.0}%)", passed, total, pct);

    // Per-category pass rate.
    let mut by_cat: BTreeMap<String, (usize, usize, u128)> = BTreeMap::new();
    for r in results {
        let entry = by_cat.entry(r.case.category.clone()).or_insert((0, 0, 0));
        entry.0 += if r.pass { 1 } else { 0 };
        entry.1 += 1;
        entry.2 += r.duration.as_millis();
    }
    println!();
    println!("by category:");
    for (cat, (p, t, total_ms)) in &by_cat {
        let avg = total_ms / *t as u128;
        let cat_pct = 100.0 * *p as f64 / *t as f64;
        println!(
            "  {:<18} {:>3}/{:<3} ({:>3.0}%)   avg {}ms",
            cat, p, t, cat_pct, avg
        );
    }

    // Confusion matrix.
    let labels = [
        "FindAction",
        "Integration",
        "Chat",
        "Memory",
        "Agent",
        "(none)",
    ];
    println!();
    println!("confusion (row = expected, col = actual):");
    print!("  {:<13}", "");
    for l in &labels {
        print!(" {:>12}", l);
    }
    println!();
    // We iterate over the five real intent labels as rows; (none) is
    // an actual outcome but never an expected value.
    for expected in &labels[..5] {
        print!("  {:<13}", expected);
        for actual in &labels {
            let count = results
                .iter()
                .filter(|r| {
                    r.case.expected_intent == *expected && intent_label(r.actual_intent) == **actual
                })
                .count();
            if count == 0 {
                print!(" {:>12}", ".");
            } else {
                print!(" {:>12}", count);
            }
        }
        println!();
    }

    // Failures detail.
    let failures: Vec<&RunResult> = results.iter().filter(|r| !r.pass).collect();
    if !failures.is_empty() {
        println!();
        println!("failures ({}):", failures.len());
        for r in &failures {
            println!(
                "  {} expected={} got={}  | {}",
                r.case.id,
                r.case.expected_intent,
                intent_label(r.actual_intent),
                r.case.transcript
            );
        }
    }

    let total_ms: u128 = results.iter().map(|r| r.duration.as_millis()).sum();
    println!();
    println!("total time: {:.1}s", total_ms as f64 / 1000.0);
}
