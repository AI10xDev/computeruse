use crate::{
    Button, ButtonState, Key, KeyState, KeyboardBackend, KeyboardEvent, MouseBackend, MouseEvent,
};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, Device, EventSummary, EventType, InputEvent, KeyCode,
    PropType, RelativeAxisCode, SynchronizationCode,
};
use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

const ABS_MAX: i32 = u16::MAX as i32;

pub struct LinuxMouse {
    relative: VirtualDevice,
    absolute: VirtualDevice,
}

impl LinuxMouse {
    pub fn new() -> io::Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for key in [
            KeyCode::BTN_LEFT,
            KeyCode::BTN_RIGHT,
            KeyCode::BTN_MIDDLE,
            KeyCode::BTN_SIDE,
            KeyCode::BTN_EXTRA,
        ] {
            keys.insert(key);
        }

        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        for axis in [
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
        ] {
            axes.insert(axis);
        }

        let relative = VirtualDevice::builder()?
            .name("computeruse virtual mouse")
            .with_keys(&keys)?
            .with_relative_axes(&axes)?
            .build()?;

        let absolute = VirtualDevice::builder()?
            .name("computeruse absolute pointer")
            .with_absolute_axis(&evdev::UinputAbsSetup::new(
                AbsoluteAxisCode::ABS_X,
                AbsInfo::new(0, 0, ABS_MAX, 0, 0, 1),
            ))?
            .with_absolute_axis(&evdev::UinputAbsSetup::new(
                AbsoluteAxisCode::ABS_Y,
                AbsInfo::new(0, 0, ABS_MAX, 0, 0, 1),
            ))?
            .build()?;

        Ok(Self { relative, absolute })
    }
}

impl MouseBackend for LinuxMouse {
    type Error = io::Error;

    fn button(&mut self, button: Button, state: ButtonState) -> Result<(), Self::Error> {
        let value = i32::from(state == ButtonState::Down);
        self.relative.emit(&[InputEvent::new(
            EventType::KEY.0,
            button_key(button).0,
            value,
        )])
    }

    fn move_relative(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
        self.relative.emit(&[
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx),
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy),
        ])
    }

    fn move_absolute(&mut self, x: u16, y: u16) -> Result<(), Self::Error> {
        self.absolute.emit(&[
            InputEvent::new(
                EventType::ABSOLUTE.0,
                AbsoluteAxisCode::ABS_X.0,
                i32::from(x),
            ),
            InputEvent::new(
                EventType::ABSOLUTE.0,
                AbsoluteAxisCode::ABS_Y.0,
                i32::from(y),
            ),
        ])
    }

    fn wheel(&mut self, delta: i32, horizontal: bool) -> Result<(), Self::Error> {
        let axis = if horizontal {
            RelativeAxisCode::REL_HWHEEL
        } else {
            RelativeAxisCode::REL_WHEEL
        };
        self.relative
            .emit(&[InputEvent::new(EventType::RELATIVE.0, axis.0, delta)])
    }
}

pub struct LinuxKeyboard {
    device: VirtualDevice,
}

impl LinuxKeyboard {
    pub fn new() -> io::Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in 1..=0x2ff {
            if is_keyboard_code(code) {
                keys.insert(KeyCode::new(code));
            }
        }
        let device = VirtualDevice::builder()?
            .name("computeruse virtual keyboard")
            .with_keys(&keys)?
            .build()?;
        Ok(Self { device })
    }

    pub fn supports_scan_code(scan_code: u16) -> bool {
        is_keyboard_code(scan_code)
    }
}

impl KeyboardBackend for LinuxKeyboard {
    type Error = io::Error;

    fn key(&mut self, scan_code: u16, state: KeyState) -> Result<(), Self::Error> {
        if !is_keyboard_code(scan_code) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("scan code {scan_code} is not a supported Linux keyboard key"),
            ));
        }
        self.device.emit(&[InputEvent::new(
            EventType::KEY.0,
            scan_code,
            i32::from(state == KeyState::Down),
        )])
    }

    fn repeat(&mut self, scan_code: u16) -> Result<(), Self::Error> {
        if !is_keyboard_code(scan_code) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("scan code {scan_code} is not a supported Linux keyboard key"),
            ));
        }
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, scan_code, 2)])
    }
}

pub struct Listener {
    devices: Vec<(PathBuf, Device, PointerState)>,
}

#[derive(Default)]
struct PointerState {
    x: Option<i32>,
    y: Option<i32>,
    pending: Vec<MouseEvent>,
}

pub struct KeyboardListener {
    devices: Vec<(PathBuf, Device)>,
}

impl KeyboardListener {
    pub fn new() -> io::Result<Self> {
        let mut devices = Vec::new();
        for (path, device) in evdev::enumerate() {
            if is_keyboard(&device) {
                device.set_nonblocking(true)?;
                devices.push((path, device));
            }
        }
        if devices.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no readable keyboard devices found under /dev/input; check permissions",
            ));
        }
        Ok(Self { devices })
    }

    pub fn device_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.devices.iter().map(|(path, _)| path)
    }

    pub fn listen(mut self, mut callback: impl FnMut(KeyboardEvent)) -> io::Result<()> {
        self.listen_until(|event| {
            callback(event);
            false
        })
    }

    pub fn listen_until(
        &mut self,
        mut callback: impl FnMut(KeyboardEvent) -> bool,
    ) -> io::Result<()> {
        loop {
            let mut had_event = false;
            for (_, device) in &mut self.devices {
                match device.fetch_events() {
                    Ok(events) => {
                        for event in events {
                            if let Some(event) = convert_keyboard_event(event) {
                                had_event = true;
                                if callback(event) {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            if !had_event {
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

impl Listener {
    pub fn new() -> io::Result<Self> {
        let mut devices = Vec::new();
        for (path, device) in evdev::enumerate() {
            if is_mouse(&device) {
                device.set_nonblocking(true)?;
                devices.push((path, device, PointerState::default()));
            }
        }
        if devices.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no readable mouse devices found under /dev/input; check permissions",
            ));
        }
        Ok(Self { devices })
    }

    pub fn device_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.devices.iter().map(|(path, _, _)| path)
    }

    pub fn listen(mut self, mut callback: impl FnMut(MouseEvent)) -> io::Result<()> {
        self.listen_until(|event| {
            callback(event);
            false
        })
    }

    /// Listens until the callback returns `true`.
    pub fn listen_until(&mut self, mut callback: impl FnMut(MouseEvent) -> bool) -> io::Result<()> {
        self.listen_until_cancelled(&mut callback, || false)
    }

    /// Listens until the callback or cancellation predicate returns `true`.
    pub fn listen_until_cancelled(
        &mut self,
        mut callback: impl FnMut(MouseEvent) -> bool,
        mut cancelled: impl FnMut() -> bool,
    ) -> io::Result<()> {
        loop {
            let mut batch = Vec::new();
            for (_, device, state) in &mut self.devices {
                match device.fetch_events() {
                    Ok(events) => {
                        for event in events {
                            batch.extend(convert_event(event, state));
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            batch.sort_by(|left, right| left.timestamp().total_cmp(&right.timestamp()));
            let had_event = !batch.is_empty();
            for event in batch {
                if callback(event) {
                    return Ok(());
                }
            }
            // Drain events already queued in this polling pass before stopping.
            if cancelled() {
                return Ok(());
            }
            if !had_event {
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

fn is_mouse(device: &Device) -> bool {
    let has_pointer_button = device.supported_keys().is_some_and(|keys| {
        [
            KeyCode::BTN_LEFT,
            KeyCode::BTN_RIGHT,
            KeyCode::BTN_MIDDLE,
            KeyCode::BTN_SIDE,
            KeyCode::BTN_EXTRA,
        ]
        .iter()
        .any(|key| keys.contains(*key))
    });
    let has_relative_position = device.supported_relative_axes().is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    });
    let has_absolute_position = device.supported_absolute_axes().is_some_and(|axes| {
        axes.contains(AbsoluteAxisCode::ABS_X) && axes.contains(AbsoluteAxisCode::ABS_Y)
    });
    has_pointer_button
        || (device.properties().contains(PropType::POINTER)
            && (has_relative_position || has_absolute_position))
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(KeyCode::KEY_A)
            && keys.contains(KeyCode::KEY_Z)
            && keys.contains(KeyCode::KEY_ENTER)
            && keys.contains(KeyCode::KEY_SPACE)
    })
}

fn is_keyboard_code(code: u16) -> bool {
    (1..=0x2ff).contains(&code) && !(0x100..=0x15f).contains(&code)
}

fn convert_keyboard_event(event: InputEvent) -> Option<KeyboardEvent> {
    let time = event
        .timestamp()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    match event.destructure() {
        EventSummary::Key(_, key, value) if is_keyboard_code(key.code()) && value >= 0 => {
            Some(KeyboardEvent {
                key: Key::from_scan_code(key.code()),
                state: if value == 0 {
                    KeyState::Up
                } else {
                    KeyState::Down
                },
                repeat: value == 2,
                time,
            })
        }
        _ => None,
    }
}

fn convert_event(event: InputEvent, state: &mut PointerState) -> Vec<MouseEvent> {
    let time = event
        .timestamp()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    match event.destructure() {
        EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
            return std::mem::take(&mut state.pending);
        }
        EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
            *state = PointerState::default();
        }
        EventSummary::Key(_, KeyCode::BTN_TOUCH, 0) => {
            state.x = None;
            state.y = None;
        }
        EventSummary::Key(_, key, value) => {
            if let Some(button) = key_button(key) {
                state.pending.push(MouseEvent::Button {
                    button,
                    state: if value == 0 {
                        ButtonState::Up
                    } else {
                        ButtonState::Down
                    },
                    time,
                });
            }
        }
        EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, value) => {
            push_movement(&mut state.pending, value, 0, time);
        }
        EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, value) => {
            push_movement(&mut state.pending, 0, value, time);
        }
        EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, delta) => {
            state.pending.push(MouseEvent::Wheel {
                delta,
                horizontal: false,
                time,
            });
        }
        EventSummary::RelativeAxis(_, RelativeAxisCode::REL_HWHEEL, delta) => {
            state.pending.push(MouseEvent::Wheel {
                delta,
                horizontal: true,
                time,
            });
        }
        EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_X, value) => {
            if let Some(previous) = state.x.replace(value) {
                push_movement(&mut state.pending, value - previous, 0, time);
            }
        }
        EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_Y, value) => {
            if let Some(previous) = state.y.replace(value) {
                push_movement(&mut state.pending, 0, value - previous, time);
            }
        }
        _ => {}
    }
    Vec::new()
}

fn push_movement(events: &mut Vec<MouseEvent>, dx: i32, dy: i32, time: f64) {
    if let Some(MouseEvent::Move {
        dx: pending_dx,
        dy: pending_dy,
        time: pending_time,
    }) = events.last_mut()
    {
        *pending_dx += dx;
        *pending_dy += dy;
        *pending_time = time;
    } else {
        events.push(MouseEvent::Move { dx, dy, time });
    }
}

fn button_key(button: Button) -> KeyCode {
    match button {
        Button::Left => KeyCode::BTN_LEFT,
        Button::Right => KeyCode::BTN_RIGHT,
        Button::Middle => KeyCode::BTN_MIDDLE,
        Button::Side => KeyCode::BTN_SIDE,
        Button::Extra => KeyCode::BTN_EXTRA,
    }
}

fn key_button(key: KeyCode) -> Option<Button> {
    match key {
        KeyCode::BTN_LEFT => Some(Button::Left),
        KeyCode::BTN_RIGHT => Some(Button::Right),
        KeyCode::BTN_MIDDLE => Some(Button::Middle),
        KeyCode::BTN_SIDE => Some(Button::Side),
        KeyCode::BTN_EXTRA => Some(Button::Extra),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::InputEvent;

    #[test]
    fn converts_linux_button_events() {
        let mut state = PointerState::default();
        let event = InputEvent::new(evdev::EventType::KEY.0, KeyCode::BTN_RIGHT.0, 1);
        assert!(convert_event(event, &mut state).is_empty());
        assert!(matches!(
            flush(&mut state).as_slice(),
            [MouseEvent::Button {
                button: Button::Right,
                state: ButtonState::Down,
                ..
            }]
        ));
    }

    #[test]
    fn ignores_unknown_linux_events() {
        let event = InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0);
        assert!(convert_event(event, &mut PointerState::default()).is_empty());
    }

    #[test]
    fn combines_pointer_axes_in_one_replay_event() {
        let mut state = PointerState::default();
        for event in [
            InputEvent::new(evdev::EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, 25),
            InputEvent::new(evdev::EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, -10),
        ] {
            assert!(convert_event(event, &mut state).is_empty());
        }
        assert!(matches!(
            flush(&mut state).as_slice(),
            [MouseEvent::Move {
                dx: 25,
                dy: -10,
                ..
            }]
        ));
    }

    #[test]
    fn touch_release_resets_absolute_pointer_position() {
        let mut state = PointerState {
            x: Some(100),
            y: Some(200),
            pending: Vec::new(),
        };
        let release = InputEvent::new(evdev::EventType::KEY.0, KeyCode::BTN_TOUCH.0, 0);
        assert!(convert_event(release, &mut state).is_empty());
        assert_eq!(state.x, None);
        assert_eq!(state.y, None);
    }

    fn flush(state: &mut PointerState) -> Vec<MouseEvent> {
        convert_event(
            InputEvent::new(
                evdev::EventType::SYNCHRONIZATION.0,
                SynchronizationCode::SYN_REPORT.0,
                0,
            ),
            state,
        )
    }

    #[test]
    fn converts_linux_keyboard_events_and_repeats() {
        let event = InputEvent::new(evdev::EventType::KEY.0, KeyCode::KEY_A.0, 2);
        assert!(matches!(
            convert_keyboard_event(event),
            Some(KeyboardEvent {
                key: Key { scan_code: 30, .. },
                state: KeyState::Down,
                repeat: true,
                ..
            })
        ));
    }
}
