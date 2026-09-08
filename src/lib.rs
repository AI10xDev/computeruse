//! Global mouse capture and injection for modern Linux desktops.
//!
//! The Linux implementation uses the kernel `evdev` and `uinput` interfaces,
//! so it does not depend on X11 and works with both Xorg and Wayland sessions.

mod agent;
mod controller;
mod event;
mod keyboard;
mod video;

#[cfg(target_os = "linux")]
mod linux;

pub use agent::{
    Action, AgentDecision, Frame, FrameSequence, FrameSource, GptAstraPolicy, Policy, Transition,
};
pub use controller::{Controller, MouseBackend, PlaybackFilter};
pub use event::{Button, ButtonState, MouseEvent};
pub use keyboard::{
    Key, KeyState, KeyboardBackend, KeyboardController, KeyboardEvent, KeyboardPlaybackFilter,
};
pub use video::extract_video_frames;

#[cfg(target_os = "linux")]
pub use linux::{KeyboardListener, LinuxKeyboard, LinuxMouse, Listener};
