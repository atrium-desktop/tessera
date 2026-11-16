//! Root integration suite.
//!
//! One test binary contains every integration module. Cargo compiles and links
//! the suite once instead of producing one executable per former crate-local
//! `tests/*.rs` target. Modules that cross a native boundary are gated by the
//! `native` feature so flux-free CI can still run the logical suites with
//! `--no-default-features`.

#[path = "../common/mod.rs"]
mod common;

mod cli;
mod config_reload;
mod desktop_settings;
mod env_conformance;
mod ipc;
mod launch_services;
mod mcp_bridge;

#[cfg(feature = "native")]
mod composite_overlay;
