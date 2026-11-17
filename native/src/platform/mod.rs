//! Windows desktop integration. Construct after `show()` has created the HWND.
//! Keep this object on the UI thread and drop it before the Slint window.
//! Preview has a separate instance identity and never registers global hotkeys.
mod policy;
mod windows;

pub use policy::DesktopCommand;
pub use windows::{DesktopPlatform, InstanceGuard};
