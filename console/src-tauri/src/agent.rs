//! Launching the peeky cursor + voice agent as a child process: finding the
//! binary across dev and shipped-bundle layouts, and spawning it with the right
//! environment (BYOK keys, routelet model dir, a path back to this console).

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::keys::apply_byok_env;

/// Candidate paths to the peeky binary, best-first: the shipped-bundle sidecar
/// (sibling of the console exe, where Tauri's `externalBin` drops it), then the
/// workspace dev layouts (`cargo tauri dev` cwd, then a workspace-root cwd).
/// Shared by `spawn_peeky` and the integrations-status shell-out.
pub(crate) fn peeky_candidates() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Sibling of the console exe (the shipped-bundle sidecar).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("peeky"));
            #[cfg(windows)]
            candidates.push(dir.join("peeky.exe"));
        }
    }

    // 2/3. Workspace dev layout.
    candidates.extend(
        [
            "../../target/debug/peeky",
            "../../target/release/peeky",
            "target/debug/peeky",
            "target/release/peeky",
        ]
        .iter()
        .map(PathBuf::from),
    );

    candidates
}

#[tauri::command]
pub fn spawn_peeky() -> Result<(), String> {
    let candidates = peeky_candidates();
    let routelet_dir = resolve_routelet_dir();

    for path in &candidates {
        if path.exists() {
            eprintln!("[console] spawning peeky from: {}", path.display());
            let mut cmd = Command::new(path);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            apply_byok_env(&mut cmd);
            if let Some(dir) = &routelet_dir {
                eprintln!("[console] PEEKY_ROUTELET_DIR={}", dir.display());
                cmd.env("PEEKY_ROUTELET_DIR", dir);
            }
            // Hand peeky our own path so it can reopen this console's sign-in
            // window when the trial wall is hit (see peeky/src/upgrade.rs).
            if let Ok(self_exe) = std::env::current_exe() {
                cmd.env("PEEKY_CONSOLE_BIN", self_exe);
            }
            #[cfg(unix)]
            cmd.process_group(0);
            if let Ok(_child) = cmd.spawn() {
                return Ok(());
            }
        }
    }

    let tried = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "peeky binary not found. Tried: {tried}. Build it with \
         `cargo build --release -p peeky --no-default-features --features winit-window,crossplatform` first."
    ))
}

/// Find the routelet ONNX model directory to pass to peeky as
/// `PEEKY_ROUTELET_DIR`. Production path is the bundled Resources dir; dev paths
/// cover `cargo tauri dev` and a workspace-root cwd. If the user has already set
/// `PEEKY_ROUTELET_DIR`, respect it. Returns `None` when no candidate exists so
/// peeky falls back to its own default and fails loud.
fn resolve_routelet_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Ok(existing) = std::env::var("PEEKY_ROUTELET_DIR") {
        let p = PathBuf::from(existing);
        if p.exists() {
            return Some(p);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // macOS .app: Contents/MacOS/<exe> -> Contents/Resources/models/routelet
            candidates.push(dir.join("../Resources/models/routelet"));
            // Linux/Windows bundles: resources next to the exe.
            candidates.push(dir.join("resources/models/routelet"));
            // Dev: target/{debug,release}/<console> -> workspace/models/routelet
            candidates.push(dir.join("../../models/routelet"));
        }
    }

    // `cargo tauri dev` cwd is console/src-tauri/; workspace-root cwd is "".
    candidates.push(PathBuf::from("../../models/routelet"));
    candidates.push(PathBuf::from("models/routelet"));

    candidates
        .into_iter()
        .find(|p| p.join("embedder.onnx").exists())
}
