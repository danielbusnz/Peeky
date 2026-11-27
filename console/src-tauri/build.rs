use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

fn main() {
    // Tauri's `externalBin` config (see tauri.conf.json) expects each
    // binary path to have a target-triple suffix appended: for a config
    // entry like "binaries/peeky", Tauri looks for "binaries/peeky-<triple>"
    // and copies it into the bundled .app/.msi/etc. as Contents/MacOS/peeky
    // (without the suffix).
    //
    // The peeky binary itself is built outside src-tauri at
    // <workspace>/target/release/peeky. We copy it into ./binaries/ with
    // the right suffix here so `cargo tauri build` finds it.
    //
    // The user must have already built peeky in release mode before
    // running `cargo tauri build`. If they haven't, we print a warning
    // and let the Tauri bundle step decide whether to fail.
    if let Err(e) = copy_peeky_sidecar() {
        println!("cargo:warning=peeky sidecar prep skipped: {e}");
    }

    if let Err(e) = skip_misplaced_dev_copies() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    tauri_build::build()
}

fn copy_peeky_sidecar() -> Result<(), String> {
    let target = env::var("TARGET").map_err(|_| "TARGET env var missing".to_string())?;

    // Workspace target dir is two levels up from src-tauri.
    let workspace_target = PathBuf::from("../../target/release");
    let source = workspace_target.join("peeky");
    if !source.exists() {
        return Err(format!(
            "{} not found. Run `cargo build --release -p peeky ...` first.",
            source.display()
        ));
    }

    // Place the suffixed copy next to tauri.conf.json so `externalBin`
    // can find it with a short relative path.
    let dest_dir = PathBuf::from("binaries");
    fs::create_dir_all(&dest_dir).map_err(|e| format!("create_dir_all binaries/: {e}"))?;
    let dest = dest_dir.join(format!("peeky-{target}"));

    fs::copy(&source, &dest).map_err(|e| format!("copy peeky: {e}"))?;

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=binaries/peeky-{target}");

    Ok(())
}

/// Stops tauri-build 2.6.3 from copying the sidecar and resources into cargo's
/// build directory.
///
/// tauri-build copies `externalBin` and `resources` next to the dev executable
/// and takes that directory to be three levels above `OUT_DIR`. Cargo 1.100
/// nests `OUT_DIR` one level deeper (`build/<pkg>/<hash>/out`), so the copies
/// aim at `target/<profile>/build` instead. There the sidecar's destination is
/// the `peeky` package's own build directory, which tauri-build tries to
/// delete as a file, and the build script panics.
///
/// When tauri-build would write there, both keys are removed from the config
/// it reads, so it copies nothing. Nothing reads those copies: the bundler
/// takes the sidecar and resources from the paths in tauri.conf.json, and
/// `agent.rs` finds the dev binary and models through its workspace
/// fallbacks. On the old layout this does nothing.
///
/// tauri-build 2.7.0 resolves the directory correctly but needs Rust 1.90.
/// Delete this function once the workspace can take it.
fn skip_misplaced_dev_copies() -> Result<(), String> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR env var missing")?);
    let assumed_profile_dir = out_dir.ancestors().nth(3);
    if assumed_profile_dir.and_then(Path::file_name) != Some(OsStr::new("build")) {
        return Ok(());
    }

    // tauri-build's copy is also what watches the sidecar and fails the build
    // when it is missing. Keep both, so a bundle run without one stops before
    // compiling and this script reruns once the sidecar exists.
    let target = env::var("TARGET").map_err(|_| "TARGET env var missing".to_string())?;
    let sidecar = Path::new("binaries").join(format!("peeky-{target}"));
    println!("cargo:rerun-if-changed={}", sidecar.display());
    if !sidecar.is_file() {
        return Err(format!("sidecar {} not found", sidecar.display()));
    }

    // TAURI_CONFIG is a JSON merge patch over tauri.conf.json in which null
    // removes a key. The tauri CLI may have set one already, so extend it.
    let mut patch: Map<String, Value> = match env::var("TAURI_CONFIG") {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| format!("parse TAURI_CONFIG: {e}"))?,
        Err(_) => Map::new(),
    };
    let bundle = patch.entry("bundle").or_insert_with(|| json!({}));
    if let Some(bundle) = bundle.as_object_mut() {
        bundle.insert("externalBin".into(), Value::Null);
        bundle.insert("resources".into(), Value::Null);
    }
    // SAFETY: nothing has spawned a thread yet, so no other code is reading
    // or writing the environment.
    unsafe { env::set_var("TAURI_CONFIG", Value::Object(patch).to_string()) };

    Ok(())
}
