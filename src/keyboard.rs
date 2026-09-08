use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub name: String,
    pub scan_code: u16,
}

impl Key {
    pub fn from_scan_code(scan_code: u16) -> Self {
        Self {
            name: key_name(scan_code),
            scan_code,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

impl FromStr for Key {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        let scan_code = parse_key_code(&normalized)
            .ok_or_else(|| format!("unknown key: {value}; use a key name or code:<number>"))?;
        Ok(Self {
            name: normalized,
            scan_code,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyboardEvent {
    pub key: Key,
    pub state: KeyState,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat: bool,
    pub time: f64,
}

impl KeyboardEvent {
    pub fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }
}

pub trait KeyboardBackend {
    type Error;

    fn key(&mut self, scan_code: u16, state: KeyState) -> Result<(), Self::Error>;

    fn repeat(&mut self, scan_code: u16) -> Result<(), Self::Error> {
        self.key(scan_code, KeyState::Down)
    }
}

pub struct KeyboardController<B> {
    backend: B,
}

impl<B: KeyboardBackend> KeyboardController<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn press(&mut self, key: &Key) -> Result<(), B::Error> {
        self.backend.key(key.scan_code, KeyState::Down)
    }

    pub fn release(&mut self, key: &Key) -> Result<(), B::Error> {
        self.backend.key(key.scan_code, KeyState::Up)
    }

    pub fn send(&mut self, keys: &[Key]) -> Result<(), B::Error> {
        for (pressed, key) in keys.iter().enumerate() {
            if let Err(error) = self.press(key) {
                for key in keys[..pressed].iter().rev() {
                    let _ = self.release(key);
                }
                return Err(error);
            }
        }
        let mut first_error = None;
        for key in keys.iter().rev() {
            if let Err(error) = self.release(key)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    pub fn send_hotkey(&mut self, hotkey: &str) -> Result<(), String>
    where
        B::Error: fmt::Display,
    {
        let keys = parse_hotkey(hotkey)?;
        self.send(&keys).map_err(|error| error.to_string())
    }

    pub fn play(
        &mut self,
        events: &[KeyboardEvent],
        speed: f64,
        filter: KeyboardPlaybackFilter,
    ) -> Result<(), B::Error> {
        let mut previous_time: Option<f64> = None;
        for event in events {
            if speed > 0.0
                && let Some(previous) = previous_time
            {
                let delay = ((event.time - previous) / speed).max(0.0);
                thread::sleep(Duration::from_secs_f64(delay));
            }
            previous_time = Some(event.time);
            if (event.state == KeyState::Down && filter.press)
                || (event.state == KeyState::Up && filter.release)
            {
                if event.repeat {
                    self.backend.repeat(event.key.scan_code)?;
                } else {
                    self.backend.key(event.key.scan_code, event.state)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KeyboardPlaybackFilter {
    pub press: bool,
    pub release: bool,
}

impl Default for KeyboardPlaybackFilter {
    fn default() -> Self {
        Self {
            press: true,
            release: true,
        }
    }
}

pub fn parse_hotkey(value: &str) -> Result<Vec<Key>, String> {
    let keys: Result<Vec<_>, _> = value.split('+').map(str::parse).collect();
    let keys = keys?;
    if keys.is_empty() {
        return Err("hotkey cannot be empty".into());
    }
    Ok(keys)
}

fn parse_key_code(name: &str) -> Option<u16> {
    if let Some(code) = name.strip_prefix("code:") {
        return code.parse().ok();
    }
    if name.len() == 1 {
        let byte = name.as_bytes()[0];
        if byte.is_ascii_lowercase() {
            const LETTER_CODES: [u16; 26] = [
                30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22,
                47, 17, 45, 21, 44,
            ];
            return Some(LETTER_CODES[(byte - b'a') as usize]);
        }
        if byte.is_ascii_digit() {
            return Some(if byte == b'0' {
                11
            } else {
                u16::from(byte - b'1') + 2
            });
        }
    }
    if let Some(number) = name
        .strip_prefix('f')
        .and_then(|value| value.parse::<u16>().ok())
        && (1..=10).contains(&number)
    {
        return Some(58 + number);
    }
    Some(match name {
        "escape" | "esc" => 1,
        "minus" => 12,
        "equal" => 13,
        "backspace" => 14,
        "tab" => 15,
        "left_brace" | "left_bracket" => 26,
        "right_brace" | "right_bracket" => 27,
        "enter" | "return" => 28,
        "left_ctrl" | "ctrl" | "control" => 29,
        "semicolon" => 39,
        "apostrophe" | "quote" => 40,
        "grave" | "backtick" => 41,
        "left_shift" | "shift" => 42,
        "backslash" => 43,
        "comma" => 51,
        "dot" | "period" => 52,
        "slash" => 53,
        "right_shift" => 54,
        "left_alt" | "alt" => 56,
        "space" => 57,
        "caps_lock" => 58,
        "num_lock" => 69,
        "scroll_lock" => 70,
        "f11" => 87,
        "f12" => 88,
        "right_ctrl" => 97,
        "right_alt" | "alt_gr" => 100,
        "home" => 102,
        "up" => 103,
        "page_up" => 104,
        "left" => 105,
        "right" => 106,
        "end" => 107,
        "down" => 108,
        "page_down" => 109,
        "insert" => 110,
        "delete" => 111,
        "mute" => 113,
        "volume_down" => 114,
        "volume_up" => 115,
        "left_meta" | "meta" | "super" | "windows" => 125,
        "right_meta" => 126,
        "menu" => 139,
        _ => return None,
    })
}

fn key_name(scan_code: u16) -> String {
    for name in [
        "escape",
        "backspace",
        "tab",
        "enter",
        "left_ctrl",
        "left_shift",
        "right_shift",
        "left_alt",
        "space",
        "caps_lock",
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "f11",
        "f12",
        "right_ctrl",
        "right_alt",
        "home",
        "up",
        "page_up",
        "left",
        "right",
        "end",
        "down",
        "page_down",
        "insert",
        "delete",
        "left_meta",
        "right_meta",
        "menu",
    ] {
        if parse_key_code(name) == Some(scan_code) {
            return name.into();
        }
    }
    for byte in b'a'..=b'z' {
        let name = char::from(byte).to_string();
        if parse_key_code(&name) == Some(scan_code) {
            return name;
        }
    }
    for byte in b'0'..=b'9' {
        let name = char::from(byte).to_string();
        if parse_key_code(&name) == Some(scan_code) {
            return name;
        }
    }
    format!("code:{scan_code}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    #[derive(Default)]
    struct FakeKeyboard(Vec<(u16, KeyState)>);

    impl KeyboardBackend for FakeKeyboard {
        type Error = Infallible;

        fn key(&mut self, scan_code: u16, state: KeyState) -> Result<(), Self::Error> {
            self.0.push((scan_code, state));
            Ok(())
        }

        fn repeat(&mut self, scan_code: u16) -> Result<(), Self::Error> {
            self.0.push((scan_code, KeyState::Down));
            Ok(())
        }
    }

    #[test]
    fn hotkey_presses_in_order_and_releases_in_reverse() {
        let mut keyboard = KeyboardController::new(FakeKeyboard::default());
        keyboard.send_hotkey("ctrl+shift+a").unwrap();
        assert_eq!(
            keyboard.backend.0,
            [
                (29, KeyState::Down),
                (42, KeyState::Down),
                (30, KeyState::Down),
                (30, KeyState::Up),
                (42, KeyState::Up),
                (29, KeyState::Up),
            ]
        );
    }

    #[test]
    fn key_events_round_trip() {
        let event = KeyboardEvent {
            key: "enter".parse().unwrap(),
            state: KeyState::Down,
            repeat: false,
            time: 1.5,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<KeyboardEvent>(&json).unwrap(), event);
    }

    #[test]
    fn playback_emits_repeat_events() {
        let event = KeyboardEvent {
            key: "a".parse().unwrap(),
            state: KeyState::Down,
            repeat: true,
            time: 1.0,
        };
        let mut keyboard = KeyboardController::new(FakeKeyboard::default());
        keyboard
            .play(&[event], 0.0, KeyboardPlaybackFilter::default())
            .unwrap();
        assert_eq!(keyboard.backend.0, [(30, KeyState::Down)]);
    }

    #[test]
    fn hotkey_releases_pressed_modifiers_after_backend_failure() {
        struct FailingKeyboard(Vec<(u16, KeyState)>);

        impl KeyboardBackend for FailingKeyboard {
            type Error = &'static str;

            fn key(&mut self, scan_code: u16, state: KeyState) -> Result<(), Self::Error> {
                if scan_code == 30 && state == KeyState::Down {
                    return Err("injection failed");
                }
                self.0.push((scan_code, state));
                Ok(())
            }
        }

        let mut keyboard = KeyboardController::new(FailingKeyboard(Vec::new()));
        assert_eq!(
            keyboard.send_hotkey("ctrl+shift+a"),
            Err("injection failed".into())
        );
        assert_eq!(
            keyboard.backend.0,
            [
                (29, KeyState::Down),
                (42, KeyState::Down),
                (42, KeyState::Up),
                (29, KeyState::Up),
            ]
        );
    }
}
