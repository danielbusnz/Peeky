//! Screen + monitor geometry capture. Implementation selected at compile time.

#[cfg(feature = "hyprland")]
mod grim;
#[cfg(feature = "hyprland")]
pub use grim::*;

#[cfg(feature = "crossplatform")]
mod xcap;
#[cfg(feature = "crossplatform")]
pub use xcap::*;
