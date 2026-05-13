//! Push-to-talk hotkey wiring. Implementation selected at compile time.

#[cfg(feature = "hyprland")]
mod unix_signals;
#[cfg(feature = "hyprland")]
pub use unix_signals::*;

#[cfg(feature = "crossplatform")]
mod global_hotkey;
#[cfg(feature = "crossplatform")]
pub use global_hotkey::*;
