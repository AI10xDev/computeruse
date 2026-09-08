//! Global mouse capture and injection for modern Linux desktops.
//!
//! The Linux implementation uses the kernel `evdev` and `uinput` interfaces,
//! so it does not depend on X11 and works with both Xorg and Wayland sessions.

mod agent;
mod controller;
mod event;
mod keyboard;
mod trajectory;
mod video;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod x11;

pub use agent::{
    Action, AgentDecision, Frame, FrameSequence, FrameSource, GptAstraPolicy, Policy, Transition,
};
pub use controller::{Controller, MouseBackend, PlaybackFilter};
pub use event::{Button, ButtonState, MouseEvent};
pub use keyboard::{
    Key, KeyState, KeyboardBackend, KeyboardController, KeyboardEvent, KeyboardPlaybackFilter,
};
pub use trajectory::{MouseTrajectory, Point, Segment, TrajectoryError, Turn};
pub use video::extract_video_frames;

#[cfg(target_os = "linux")]
pub use linux::{KeyboardListener, LinuxKeyboard, LinuxMouse, Listener};
#[cfg(target_os = "linux")]
pub use x11::{CursorLocation, X11ButtonListener, X11Cursor};
