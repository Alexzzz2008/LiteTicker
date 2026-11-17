pub mod config;
pub mod market;
pub mod model;
pub mod session;
pub mod update;

#[cfg(all(windows, feature = "gui"))]
pub mod platform;

pub const BASELINE_VERSION: &str = "1.0.5";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PRODUCT_NAME: &str = "轻量看盘";
