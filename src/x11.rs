use anyhow::{Context, Result, ensure};
use std::fmt;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, Window};

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
