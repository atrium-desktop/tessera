//! Built-in shell surfaces, compiled independently through features.
#[cfg(feature = "chrome-control-center")]
pub mod control_center;
#[cfg(feature = "chrome-dock")]
pub mod dock;
#[cfg(feature = "chrome-hud")]
pub mod hud;
#[cfg(feature = "chrome-pivot")]
pub mod pivot;
#[cfg(feature = "chrome-control-center")]
pub mod settings;
