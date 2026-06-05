use std::env;
use std::fs;
use std::path::PathBuf;

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
