use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Tauri's `externalBin` config (see tauri.conf.json) expects each
    // binary path to have a target-triple suffix appended: for a config
    // entry like "binaries/aegis", Tauri looks for "binaries/aegis-<triple>"
    // and copies it into the bundled .app/.msi/etc. as Contents/MacOS/aegis
    // (without the suffix).
    //
    // The aegis binary itself is built outside src-tauri at
    // <workspace>/target/release/aegis. We copy it into ./binaries/ with
    // the right suffix here so `cargo tauri build` finds it.
    //
    // The user must have already built aegis in release mode before
    // running `cargo tauri build`. If they haven't, we print a warning
    // and let the Tauri bundle step decide whether to fail.
    if let Err(e) = copy_aegis_sidecar() {
        println!("cargo:warning=aegis sidecar prep skipped: {e}");
    }

    tauri_build::build()
}

fn copy_aegis_sidecar() -> Result<(), String> {
    let target = env::var("TARGET").map_err(|_| "TARGET env var missing".to_string())?;

    // Workspace target dir is two levels up from src-tauri.
    let workspace_target = PathBuf::from("../../target/release");
    let source = workspace_target.join("aegis");
    if !source.exists() {
        return Err(format!(
            "{} not found. Run `cargo build --release -p aegis ...` first.",
            source.display()
        ));
    }

    // Place the suffixed copy next to tauri.conf.json so `externalBin`
    // can find it with a short relative path.
    let dest_dir = PathBuf::from("binaries");
    fs::create_dir_all(&dest_dir).map_err(|e| format!("create_dir_all binaries/: {e}"))?;
    let dest = dest_dir.join(format!("aegis-{target}"));

    fs::copy(&source, &dest).map_err(|e| format!("copy aegis: {e}"))?;

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=binaries/aegis-{target}");

    Ok(())
}
