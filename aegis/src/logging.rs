//! Tee process stdout and stderr to a log file while keeping terminal output.
//!
//! aegis emits every diagnostic with plain `println!`/`eprintln!`. Launched
//! from the macOS .app bundle (or any GUI launcher) the process has no
//! controlling terminal, so that output is discarded and a release build gives
//! no way to see what it is doing. [`init`] splices a pipe under fds 1 and 2
//! before anything else runs, so a reader thread copies every line to both the
//! original destination (a terminal, when present) and a persistent file.
//! Wrapping the fds rather than the print macros captures all existing output
//! untouched, including panic messages written to stderr.

use std::path::PathBuf;

/// Current log location: `<config_dir>/aegis/logs/aegis.log`
/// (`~/Library/Application Support/aegis/logs/aegis.log` on macOS,
/// `~/.config/aegis/logs/aegis.log` on Linux). None if the config dir can't
/// be resolved.
fn log_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("aegis")
            .join("logs")
            .join("aegis.log"),
    )
}

/// Begin teeing stdout and stderr to the log file. Call once as the first line
/// of `main`, before any output. Returns the log path on success; logging is
/// best effort, so a failure to set it up returns None and leaves the streams
/// untouched rather than aborting startup.
#[cfg(unix)]
pub fn init() -> Option<PathBuf> {
    use std::fs::{self, File};

    let path = log_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    // Keep the previous run for post-mortem and start each launch fresh, so a
    // single log never grows without bound. Renaming a missing prior log (the
    // first run) is fine, hence the ignored result.
    let _ = fs::rename(&path, path.with_extension("log.old"));
    let file = File::create(&path).ok()?;

    // Both streams land in the same file: stderr carries panics and the
    // [startup] lines, stdout the per-turn pipeline logs.
    tee(libc::STDOUT_FILENO, file.try_clone().ok()?)?;
    tee(libc::STDERR_FILENO, file)?;
    Some(path)
}

/// Windows lacks the dup2/pipe fd plumbing this uses; release logging there can
/// grow a console-handle equivalent when needed. No-op for now.
#[cfg(not(unix))]
pub fn init() -> Option<PathBuf> {
    None
}

/// Redirect `target_fd` through a pipe whose reader fans each line out to the
/// fd's original destination and to `file`. Spawns one detached reader thread
/// that lives for the rest of the process.
#[cfg(unix)]
fn tee(target_fd: i32, mut file: std::fs::File) -> Option<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::fd::FromRawFd;

    // SAFETY: dup/pipe/dup2/close act on valid descriptors; every return value
    // is checked, and each raw fd we keep is immediately wrapped in an owning
    // File so it is closed exactly once. target_fd (1 or 2) stays open for the
    // whole process.
    let original_fd = unsafe { libc::dup(target_fd) };
    if original_fd < 0 {
        return None;
    }
    let mut original = unsafe { std::fs::File::from_raw_fd(original_fd) };

    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);

    // Point the target fd at the pipe's write end, then drop our copy of the
    // write end so the reader sees EOF once the process and its threads exit.
    if unsafe { libc::dup2(write_fd, target_fd) } < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return None;
    }
    unsafe { libc::close(write_fd) };
    let reader = unsafe { std::fs::File::from_raw_fd(read_fd) };

    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    // Echo to the original stream first so live terminal runs
                    // are unchanged, then persist. Both writes are best effort.
                    let _ = original.write_all(&line);
                    let _ = original.flush();
                    let _ = file.write_all(&line);
                    let _ = file.flush();
                }
            }
        }
    });
    Some(())
}
