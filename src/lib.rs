//! Global mouse capture and injection for modern Linux desktops.
//!
//! The Linux implementation uses the kernel `evdev` and `uinput` interfaces,
//! so raw input works with both Xorg and Wayland sessions. Measured proportional
//! cursor control additionally requires native X11 feedback.

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
pub use controller::{
    Controller, CursorBackend, CursorMotion, MouseBackend, PlaybackFilter, move_cursor_to,
};
pub use event::{Button, ButtonState, MouseEvent};
pub use keyboard::{
    Key, KeyState, KeyboardBackend, KeyboardController, KeyboardEvent, KeyboardPlaybackFilter,
};
pub use trajectory::{CursorEstimate, MouseTrajectory, Point, Segment, TrajectoryError, Turn};
pub use video::{extract_video_frames, extract_video_frames_sampled};

#[cfg(target_os = "linux")]
pub use linux::{KeyboardListener, LinuxKeyboard, LinuxMouse, Listener};
#[cfg(target_os = "linux")]
pub use x11::{CursorLocation, X11ButtonListener, X11Cursor};
