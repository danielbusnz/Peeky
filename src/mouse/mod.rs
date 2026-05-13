//! Mouse position polling. Implementation selected at compile time.

#[cfg(feature = "hyprland")]
mod hyprland;
#[cfg(feature = "hyprland")]
pub use hyprland::*;

#[cfg(feature = "crossplatform")]
mod crossplatform;
#[cfg(feature = "crossplatform")]
pub use crossplatform::*;
