//! Windows desktop integration. Construct after `show()` has created the HWND.
//! Keep this object on the UI thread and drop it before the Slint window.
//! Preview uses its own instance identity. Offline replay skips hotkey registration.
mod policy;
mod windows;

pub use policy::DesktopCommand;
pub use windows::{DesktopPlatform, InstanceGuard};
