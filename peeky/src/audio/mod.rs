//! Audio I/O. Mic input on one side, speaker output on the other; the two
//! sides share no state and are kept in separate modules. Re-exported flatly
//! so callers use `audio::Mic`, `audio::AudioOutput`, etc.

mod input;
mod output;

// Examples (test_stt, test_stt_bench) include this module via #[path]
// but only use the input side, so the output re-export looks unused
// from their compile units. The aegis binary uses both.
#[allow(unused_imports)]
pub use input::*;
#[allow(unused_imports)]
pub use output::*;
