// Smoke test for the local ONNX intent classifier (routelet).
//
// Run with: cargo run -p aegis-demos --bin demo_routelet
//
// Loads the model from models/routelet (relative to the repo root, where
// cargo run places the working directory). Classifies five phrases, prints
// the predicted intent, and checks it against the expected label.

use aegis::routelet::Routelet;
use std::path::Path;

struct Case {
    phrase: &'static str,
    expected: &'static str,
}

fn main() {
    let dir = Path::new("models/routelet");
    let t_load = std::time::Instant::now();
    let routelet = Routelet::load(dir)
        .unwrap_or_else(|e| panic!("failed to load routelet from {}: {e}", dir.display()));
    let load_ms = t_load.elapsed().as_millis();
    println!("model loaded in {}ms\n", load_ms);

    let cases = [
        Case {
            phrase: "play despacito on spotify",
            expected: "Integration",
        },
        Case {
            phrase: "what's my wifi password",
            expected: "Memory",
        },
        Case {
            phrase: "where is the search bar",
            expected: "FindAction",
        },
        Case {
            phrase: "what's the capital of france",
            expected: "Chat",
        },
        Case {
            phrase: "open youtube, search for lofi and play the first result",
            expected: "Agent",
        },
    ];

    let mut passed = 0usize;
    let total = cases.len();

    for case in &cases {
        let t = std::time::Instant::now();
        let result = routelet.classify_with_confidence(case.phrase);
        let elapsed = t.elapsed();
        let (predicted, conf_str) = match result {
            Some((intent, conf)) => (format!("Some({intent:?})"), format!("{conf:.3}")),
            None => ("None".to_string(), "n/a".to_string()),
        };
        // The expected strings match the Debug repr of the Intent variants.
        let ok = predicted == format!("Some({})", case.expected);
        let verdict = if ok { "PASS" } else { "FAIL" };
        if ok {
            passed += 1;
        }
        println!(
            "[{}] \"{}\"\n      expected={} got={} conf={} ({:.1}ms)",
            verdict,
            case.phrase,
            case.expected,
            predicted,
            conf_str,
            elapsed.as_secs_f64() * 1000.0,
        );
    }

    println!("\n{}/{} passed", passed, total);
    if passed < total {
        std::process::exit(1);
    }
}
