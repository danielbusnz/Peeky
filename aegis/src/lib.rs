//! Aegis: a voice-controlled AI cursor.
//!
//! The crate is split into a library (this file) and a thin binary
//! (`main.rs`). The library exposes every subsystem as a public module so
//! both the binary and the out-of-tree `demos` crate build against one set
//! of modules instead of splicing `src/` files in via `#[path]`.

pub mod actions;
pub mod ai_cursor;
pub mod audio;
pub mod barge_in;
pub mod desktop;
pub mod hotkey;
pub mod input;
pub mod integrations;
pub mod intent;
pub mod mouse_position;
pub mod orchestrator;
pub mod painter;
pub mod providers;
pub mod screenshot;
pub mod tuning;
pub mod voice_session;
