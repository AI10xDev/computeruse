use crate::{Button, ButtonState, CursorBackend, MouseEvent, Point};
use anyhow::{Context, Result, ensure};
use std::fmt;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, MapState, Window,
};

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
        Self::connect_screen(Some(screen))
    }

    /// Connects to the screen selected by DISPLAY rather than assuming screen 0.
    pub fn connect_default() -> Result<Self> {
        Self::connect_screen(None)
    }

    fn connect_screen(screen: Option<usize>) -> Result<Self> {
        let (connection, default_screen) =
            x11rb::connect(None).context("cannot connect to X11; check DISPLAY and Xauthority")?;
        let screen = screen.unwrap_or(default_screen);
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

    /// Checks the connected server before enabling global proportional control.
    pub fn require_native_x11(&self) -> Result<()> {
        let extensions = self
            .connection
            .list_extensions()
            .context("cannot request X11 extensions for session detection")?
            .reply()
            .context("cannot query X11 extensions for session detection")?;
        let has_extension = |name: &[u8]| {
            extensions
                .names
                .iter()
                .any(|extension| extension.name == name)
        };
        validate_native_x11(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            has_extension(b"XWAYLAND"),
            has_extension(b"XFree86-DGA"),
        )
    }

    /// Activates an exact, ASCII-case-insensitive WM_CLASS instance or class match.
    /// An active match wins; multiple inactive matches are an error. Success requires
    /// the window manager to report the target as both active and viewable.
    pub fn focus_window(&mut self, class: &str) -> Result<()> {
        ensure!(
            !class.is_empty() && !class.contains('\0'),
            "window class must be nonempty and contain no NUL bytes"
        );
        let atom = |name: &[u8]| -> Result<u32> {
            let atom = self.connection.intern_atom(true, name)?.reply()?.atom;
            ensure!(
                atom != x11rb::NONE,
                "window manager does not provide EWMH {}",
                String::from_utf8_lossy(name)
            );
            Ok(atom)
        };
        let client_list = atom(b"_NET_CLIENT_LIST")?;
        let active_window = atom(b"_NET_ACTIVE_WINDOW")?;
        let windows = |property, name: &str| -> Result<Vec<Window>> {
            let reply = self
                .connection
                .get_property(false, self.root, property, AtomEnum::WINDOW, 0, u32::MAX)?
                .reply()
                .with_context(|| format!("cannot read EWMH {name}"))?;
            ensure!(
                reply.type_ == u32::from(AtomEnum::WINDOW) && reply.format == 32,
                "window manager has missing or invalid EWMH {name}"
            );
            Ok(reply.value32().unwrap().collect())
        };
        let active = || -> Result<Window> {
            let value = windows(active_window, "_NET_ACTIVE_WINDOW")?;
            ensure!(value.len() == 1, "invalid EWMH _NET_ACTIVE_WINDOW length");
            Ok(value[0])
        };
        let mut matches = Vec::new();
        for window in windows(client_list, "_NET_CLIENT_LIST")? {
            let reply = self
                .connection
                .get_property(
                    false,
                    window,
                    AtomEnum::WM_CLASS,
                    AtomEnum::STRING,
                    0,
                    u32::MAX,
                )?
                .reply()
                .with_context(|| format!("cannot read WM_CLASS for window {window:#x}"))?;
            if reply.type_ == u32::from(AtomEnum::STRING)
                && reply.format == 8
                && wm_class_matches(&reply.value, class)
            {
                matches.push(window);
            }
        }
        let current = active()?;
        let target = matching_window(&matches, current, class)?;
        if let Some(request) = activation_request(target, active_window, current) {
            self.connection
                .send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    request,
                )
                .context("cannot send EWMH window activation request")?
                .check()
                .context("X11 rejected EWMH window activation request")?;
            self.connection
                .flush()
                .context("cannot flush EWMH window activation request")?;
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let attributes = self
                .connection
                .get_window_attributes(target)?
                .reply()
                .with_context(|| {
                    format!("cannot inspect activation target {target:#x} ({class:?})")
                })?;
            if active()? == target && attributes.map_state == MapState::VIEWABLE {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for window {target:#x} ({class:?}) to become active and viewable; window manager did not acknowledge activation"
            );
            thread::sleep(Duration::from_millis(20));
        }
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
        self.warp(x, y)?;
        let location = self.location()?;
        ensure!(
            location.x == x && location.y == y,
            "X11 pointer calibration failed: requested ({x}, {y}), got ({}, {})",
            location.x,
            location.y
        );
        Ok(location)
    }

    fn warp(&self, x: i16, y: i16) -> Result<()> {
        self.connection
            .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, x, y)
            .context("cannot request X11 pointer warp")?
            .check()
            .context("X11 rejected pointer warp")?;
        self.connection
            .flush()
            .context("cannot flush X11 pointer warp")
    }
}

fn wm_class_matches(value: &[u8], class: &str) -> bool {
    !class.is_empty()
        && value
            .split(|byte| *byte == 0)
            .take(2)
            .any(|field| field.eq_ignore_ascii_case(class.as_bytes()))
}

fn matching_window(matches: &[Window], active: Window, class: &str) -> Result<Window> {
    if matches.contains(&active) {
        return Ok(active);
    }
    ensure!(
        !matches.is_empty(),
        "no managed X11 window matches WM_CLASS {class:?}"
    );
    ensure!(
        matches.len() == 1,
        "ambiguous WM_CLASS {class:?}: {} windows match and none is active; use a unique instance or class",
        matches.len()
    );
    Ok(matches[0])
}

fn activation_request(target: Window, atom: u32, active: Window) -> Option<ClientMessageEvent> {
    // Pager source (2), CurrentTime, and the currently active window, per EWMH.
    (target != active)
        .then(|| ClientMessageEvent::new(32, target, atom, [2, x11rb::CURRENT_TIME, active, 0, 0]))
}

fn validate_native_x11(
    session_type: Option<&str>,
    xwayland: bool,
    xfree86_dga: bool,
) -> Result<()> {
    ensure!(
        !xwayland,
        "proportional cursor control requires native X11, but DISPLAY points to XWayland; --mouse-control direct opts into unverified uinput movement without global cursor feedback"
    );
    // Xorg registers XFree86-DGA; Xwayland does not. Absence of the newer
    // XWAYLAND extension alone is not enough to identify native X11.
    ensure!(
        xfree86_dga || session_type == Some("x11"),
        "cannot verify native X11 for proportional cursor control: XDG_SESSION_TYPE={}, and DISPLAY does not advertise XFree86-DGA; if this display is confirmed to be native Xorg, rerun with XDG_SESSION_TYPE=x11; otherwise --mouse-control direct opts into unverified uinput movement",
        session_type.unwrap_or("<unset>")
    );
    Ok(())
}

impl CursorBackend for X11Cursor {
    fn focus_window(&mut self, class: &str) -> Result<()> {
        X11Cursor::focus_window(self, class)
    }

    fn dimensions(&mut self) -> Result<(u16, u16)> {
        let geometry = self.connection.get_geometry(self.root)?.reply()?;
        ensure!(
            geometry.width > 0
                && geometry.height > 0
                && geometry.width <= 32_768
                && geometry.height <= 32_768,
            "X11 desktop dimensions exceed the supported pointer coordinate range"
        );
        Ok((geometry.width, geometry.height))
    }

    fn position(&mut self) -> Result<Point> {
        let location = self.location()?;
        Ok(Point {
            x: i32::from(location.x),
            y: i32::from(location.y),
        })
    }

    fn move_to(&mut self, position: Point) -> Result<()> {
        self.warp(
            position
                .x
                .try_into()
                .context("cursor x exceeds X11 coordinate range")?,
            position
                .y
                .try_into()
                .context("cursor y exceeds X11 coordinate range")?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exact_wm_class_instance_or_class_ignoring_ascii_case() {
        for class in ["firefox", "FIREFOX", "Navigator", "navigator"] {
            assert!(wm_class_matches(b"firefox\0Navigator\0", class));
        }
        assert!(wm_class_matches(b"firefox\0firefox\0", "Firefox"));
        for class in ["", "fire", "fox", "firefox-esr", " firefox", "Other"] {
            assert!(!wm_class_matches(b"firefox\0Navigator\0", class));
        }
        assert!(!wm_class_matches(b"other\0Other\0firefox\0", "firefox"));
        assert!(!wm_class_matches(b"", "firefox"));
    }

    #[test]
    fn selects_active_match_or_unique_match_but_never_arbitrary_window() {
        assert_eq!(matching_window(&[10, 20], 20, "firefox").unwrap(), 20);
        assert_eq!(matching_window(&[10], 30, "firefox").unwrap(), 10);
        assert!(
            matching_window(&[], 30, "firefox")
                .unwrap_err()
                .to_string()
                .contains("no managed X11 window")
        );
        for matches in [&[10, 20][..], &[20, 10][..]] {
            let error = matching_window(matches, 30, "firefox")
                .unwrap_err()
                .to_string();
            assert!(error.contains("ambiguous WM_CLASS \"firefox\""));
        }
    }

    #[test]
    fn activation_is_idempotent_and_uses_ewmh_pager_request() {
        assert!(activation_request(10, 99, 10).is_none());
        for active in [x11rb::NONE, 20] {
            let request = activation_request(10, 99, active).unwrap();
            assert_eq!(
                request.response_type,
                x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT
            );
            assert_eq!(request.format, 32);
            assert_eq!(request.window, 10);
            assert_eq!(request.type_, 99);
            assert_eq!(
                request.data.as_data32(),
                [2, x11rb::CURRENT_TIME, active, 0, 0]
            );
        }
    }

    #[test]
    fn native_x11_detection_handles_stale_and_missing_session_metadata() {
        for session_type in [Some("x11"), Some("wayland"), Some("tty"), Some(""), None] {
            for (xwayland, xfree86_dga) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let result = validate_native_x11(session_type, xwayland, xfree86_dga);
                assert_eq!(
                    result.is_ok(),
                    !xwayland && (xfree86_dga || session_type == Some("x11")),
                    "session={session_type:?}, XWAYLAND={xwayland}, XFree86-DGA={xfree86_dga}"
                );
            }
        }
    }

    #[test]
    fn unverified_session_errors_report_the_session_value_and_remedy() {
        for session_type in [Some("wayland"), None] {
            let error = validate_native_x11(session_type, false, false)
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!(
                "XDG_SESSION_TYPE={}",
                session_type.unwrap_or("<unset>")
            )));
            assert!(error.contains("confirmed to be native Xorg"));
            assert!(error.contains("--mouse-control direct"));
        }
        let error = validate_native_x11(Some("x11"), true, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("DISPLAY points to XWayland"));
    }

    #[test]
    #[ignore = "requires a native X11 desktop with working DISPLAY and Xauthority"]
    fn native_x11_preflight_reads_display_without_injecting_input() {
        let mut cursor = X11Cursor::connect_default().unwrap();
        cursor.require_native_x11().unwrap();
        let (width, height) = cursor.dimensions().unwrap();
        let position = cursor.position().unwrap();
        eprintln!("native X11 preflight: {width}x{height}, cursor {position:?}");
    }

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
