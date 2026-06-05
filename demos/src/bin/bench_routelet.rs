// Latency benchmark for the local routelet ONNX intent classifier.
//
// Loads the model once, warms up, then times many single-string classifies and
// reports the distribution. Run in RELEASE for representative numbers: tract is
// far slower unoptimized, so a debug build is not the production latency.
//   cargo run --release -p peeky-demos --bin bench_routelet

use peeky::routelet::Routelet;
use std::path::Path;

fn main() {
    let dir = Path::new("models/routelet");
    let t_load = std::time::Instant::now();
    let routelet = Routelet::load(dir)
        .unwrap_or_else(|e| panic!("failed to load routelet from {}: {e}", dir.display()));
    println!("model loaded in {:?}\n", t_load.elapsed());

    // A spread of realistic transcripts so we are not timing one cached path.
    let phrases = [
        "play despacito on spotify",
        "what's my wifi password",
        "where is the search bar",
        "what's the capital of france",
        "open youtube, search for lofi and play the first result",
        "remember that i parked on level 3",
        "skip to the next song",
        "do you see the green button",
    ];

    // Warm up: the first inferences pay tract graph-optimization and allocation
    // costs that do not recur, so they would skew the distribution.
    for p in &phrases {
        let _ = routelet.classify_with_confidence(p);
    }

    let iterations = 300usize;
    let mut samples_us: Vec<u128> = Vec::with_capacity(iterations);
    for i in 0..iterations {
        let p = phrases[i % phrases.len()];
        let t = std::time::Instant::now();
        let _ = routelet.classify_with_confidence(p);
        samples_us.push(t.elapsed().as_micros());
    }

    samples_us.sort_unstable();
    let n = samples_us.len();
    let pct = |q: f64| samples_us[(((q * n as f64) as usize).max(1) - 1).min(n - 1)];
    let mean_us = samples_us.iter().sum::<u128>() as f64 / n as f64;
    let ms = |us: u128| us as f64 / 1000.0;

    println!("routelet inference latency over {n} warm runs:");
    println!("  min   {:.2} ms", ms(samples_us[0]));
    println!("  p50   {:.2} ms", ms(pct(0.50)));
    println!("  p95   {:.2} ms", ms(pct(0.95)));
    println!("  p99   {:.2} ms", ms(pct(0.99)));
    println!("  max   {:.2} ms", ms(samples_us[n - 1]));
    println!("  mean  {:.2} ms", mean_us / 1000.0);
}
