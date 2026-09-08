use crate::{Button, ButtonState, MouseEvent};
use anyhow::{Context, Result, ensure};
use std::fmt;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt, Window};

const POLL_INTERVAL: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorLocation {
    pub x: i16,
    pub y: i16,
    pub screen: usize,
    pub window: Window,
}

impl fmt::Display for CursorLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "x:{} y:{} screen:{} window:{}",
            self.x, self.y, self.screen, self.window
        )
    }
}

pub struct X11Cursor {
    connection: x11rb::rust_connection::RustConnection,
    screen: usize,
    root: Window,
}

pub struct X11ButtonListener {
    connection: x11rb::rust_connection::RustConnection,
}

impl X11ButtonListener {
    pub fn connect(screen: usize) -> Result<Self> {
        let (connection, _) =
            x11rb::connect(None).context("cannot connect to X11; check DISPLAY and Xauthority")?;
        let root = connection
            .setup()
            .roots
            .get(screen)
            .with_context(|| format!("X11 screen {screen} does not exist"))?
            .root;
        let version = connection
            .xinput_xi_query_version(2, 1)
            .context("cannot query XInput version")?
            .reply()
            .context("XInput 2 is unavailable")?;
        ensure!(
            version.major_version > 2 || (version.major_version == 2 && version.minor_version >= 1),
            "XInput 2.1 or newer is required, server provides {}.{}",
            version.major_version,
            version.minor_version
        );

        let masks = [xinput::EventMask {
            deviceid: u16::from(xinput::Device::ALL_MASTER),
            mask: vec![
                xinput::XIEventMask::RAW_BUTTON_PRESS | xinput::XIEventMask::RAW_BUTTON_RELEASE,
            ],
        }];
        connection
            .xinput_xi_select_events(root, &masks)
            .context("cannot select XInput button events")?
            .check()
            .context("X11 rejected XInput button event selection")?;
        connection
            .flush()
            .context("cannot flush XInput button event selection")?;

        Ok(Self { connection })
    }

    pub fn listen_until_cancelled(
        &self,
        mut callback: impl FnMut(MouseEvent) -> bool,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<()> {
        loop {
            let mut had_event = false;
            while let Some(event) = self
                .connection
                .poll_for_event()
                .context("cannot poll XInput button events")?
            {
                if let Some(event) = convert_button_event(event) {
                    had_event = true;
                    if callback(event) {
                        return Ok(());
                    }
                }
            }
            if cancelled() {
                return Ok(());
            }
            if !had_event {
                thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

fn convert_button_event(event: Event) -> Option<MouseEvent> {
    let (detail, state) = match event {
        Event::XinputRawButtonPress(event) => (event.detail, ButtonState::Down),
        Event::XinputRawButtonRelease(event) => (event.detail, ButtonState::Up),
        _ => return None,
    };
    let button = x11_button(detail)?;
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    Some(MouseEvent::Button {
        button,
        state,
        time,
    })
}

fn x11_button(detail: u32) -> Option<Button> {
    match detail {
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        8 => Some(Button::Side),
        9 => Some(Button::Extra),
        _ => None,
    }
}

impl X11Cursor {
    pub fn connect(screen: usize) -> Result<Self> {
        let (connection, _) =
            x11rb::connect(None).context("cannot connect to X11; check DISPLAY and Xauthority")?;
        let root = connection
            .setup()
            .roots
            .get(screen)
            .with_context(|| format!("X11 screen {screen} does not exist"))?
            .root;
        Ok(Self {
            connection,
            screen,
            root,
        })
    }

    /// X11 equivalent of `xdotool getmouselocation`.
    pub fn location(&self) -> Result<CursorLocation> {
        let reply = self
            .connection
            .query_pointer(self.root)
            .context("cannot send X11 pointer query")?
            .reply()
            .context("X11 pointer query failed")?;
        ensure!(
            reply.same_screen,
            "pointer is not on X11 screen {}",
            self.screen
        );
        Ok(CursorLocation {
            x: reply.root_x,
            y: reply.root_y,
            screen: self.screen,
            window: if reply.child == x11rb::NONE {
                self.root
            } else {
                reply.child
            },
        })
    }

    pub fn home(&self, x: i16, y: i16) -> Result<CursorLocation> {
        let screen = &self.connection.setup().roots[self.screen];
        ensure!(
            x >= 0
                && y >= 0
                && i32::from(x) < i32::from(screen.width_in_pixels)
                && i32::from(y) < i32::from(screen.height_in_pixels),
            "home position ({x}, {y}) is outside X11 screen {} ({}x{})",
            self.screen,
            screen.width_in_pixels,
            screen.height_in_pixels
        );
        self.connection
            .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, x, y)
            .context("cannot request X11 pointer warp")?
            .check()
            .context("X11 rejected pointer warp")?;
        self.connection
            .flush()
            .context("cannot flush X11 pointer warp")?;

        let location = self.location()?;
        ensure!(
            location.x == x && location.y == y,
            "X11 pointer calibration failed: requested ({x}, {y}), got ({}, {})",
            location.x,
            location.y
        );
        Ok(location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_xinput_buttons_and_ignores_wheel_details() {
        assert_eq!(x11_button(1), Some(Button::Left));
        assert_eq!(x11_button(2), Some(Button::Middle));
        assert_eq!(x11_button(3), Some(Button::Right));
        assert_eq!(x11_button(8), Some(Button::Side));
        assert_eq!(x11_button(9), Some(Button::Extra));
        for detail in 4..=7 {
            assert_eq!(x11_button(detail), None);
        }
    }
}
