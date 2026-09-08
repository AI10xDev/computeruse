//! Global mouse capture and injection for modern Linux desktops.
//!
//! The Linux implementation uses the kernel `evdev` and `uinput` interfaces,
//! so it does not depend on X11 and works with both Xorg and Wayland sessions.

mod controller;
mod event;

#[cfg(target_os = "linux")]
mod linux;

pub use controller::{Controller, MouseBackend, PlaybackFilter};
pub use event::{Button, ButtonState, MouseEvent};

#[cfg(target_os = "linux")]
pub use linux::{LinuxMouse, Listener};
