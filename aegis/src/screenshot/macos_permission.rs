//! macOS Screen Recording (TCC) permission for the capture process.
//!
//! aegis is the process that screenshots the screen, and after the launcher
//! exits it becomes its own TCC "responsible process", so the grant has to land
//! on aegis itself (the embedded Info.plist in `main.rs` gives it a stable
//! identity to hold that grant). macOS never grants screen recording
//! programmatically: the user toggles it in System Settings, and the grant only
//! takes effect on the *next* launch of the process. A denied process keeps
//! getting a wallpaper-only frame for its whole lifetime, with no error.
//!
//! So when access is missing we surface the system prompt, then poll until the
//! user flips the switch and re-exec ourselves so the new instance starts
//! authorized.

// How often the watcher re-checks for the grant. Structural, not a voice dial:
// it only governs how fast aegis self-relaunches after the user toggles the
// setting, so a couple of seconds is imperceptible and costs nothing.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

// CoreGraphics screen-capture access APIs (macOS 10.15+). Not surfaced by
// objc2-core-graphics; the framework is already linked, so a bare declaration
// binds them.
unsafe extern "C" {
    // Whether this process currently has Screen Recording access. No prompt.
    fn CGPreflightScreenCaptureAccess() -> bool;
    // Adds the process to the Screen Recording list and shows the one-time
    // system prompt on first call. Returns the current grant status.
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// True if this process can currently capture other apps' windows.
pub fn has_access() -> bool {
    // SAFETY: parameterless CoreGraphics C call, always sound to invoke.
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Ensure Screen Recording access, prompting and self-relaunching if needed.
///
/// Returns immediately when access is already granted (the common case, one
/// cheap syscall). Otherwise it triggers the system prompt and spawns a watcher
/// thread that polls until the user grants it, then re-execs the process so the
/// new instance starts authorized. The caller keeps running meanwhile, since
/// chat and other non-visual paths work fine without screen access.
pub fn ensure_access() {
    if has_access() {
        eprintln!("[permission] screen recording already granted");
        return;
    }
    // SAFETY: parameterless CoreGraphics C call. Shows the prompt and adds the
    // Screen Recording entry on the first call; a no-op once the user decided.
    unsafe { CGRequestScreenCaptureAccess() };
    eprintln!(
        "[permission] screen recording NOT granted. Enable Aegis under System \
         Settings -> Privacy & Security -> Screen Recording; aegis will relaunch \
         itself once you do."
    );
    std::thread::spawn(watch_and_relaunch);
}

/// Poll for the grant, then replace this process so capture works. The
/// preflight result flips the instant the user toggles the setting, even though
/// the live process stays blind until relaunch, so it is the signal we wait on.
fn watch_and_relaunch() {
    loop {
        std::thread::sleep(POLL_INTERVAL);
        if has_access() {
            relaunch();
        }
    }
}

/// Re-exec the current binary with the same args and environment. On success it
/// never returns (the process image is replaced). On failure it logs and
/// returns, leaving the watcher to keep polling so a transient error doesn't
/// strand the process blind.
fn relaunch() {
    use std::os::unix::process::CommandExt;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[permission] relaunch skipped, current_exe failed: {e}");
            return;
        }
    };
    eprintln!("[permission] screen recording granted; relaunching aegis...");
    // exec replaces the image; it only returns if it failed to do so.
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("[permission] relaunch exec failed: {err}");
}
