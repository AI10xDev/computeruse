use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    Side,
    Extra,
}

impl fmt::Display for Button {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for Button {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "middle" => Ok(Self::Middle),
            "side" | "x" => Ok(Self::Side),
            "extra" | "x2" => Ok(Self::Extra),
            _ => Err(format!("unknown mouse button: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonState {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MouseEvent {
    Button {
        button: Button,
        state: ButtonState,
        time: f64,
    },
    Move {
        dx: i32,
        dy: i32,
        time: f64,
    },
    Wheel {
        delta: i32,
        horizontal: bool,
        time: f64,
    },
}

impl MouseEvent {
    pub fn timestamp(&self) -> f64 {
        match self {
            Self::Button { time, .. } | Self::Move { time, .. } | Self::Wheel { time, .. } => *time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_aliases_parse() {
        assert_eq!("x".parse(), Ok(Button::Side));
        assert_eq!("x2".parse(), Ok(Button::Extra));
        assert!("primary".parse::<Button>().is_err());
    }

    #[test]
    fn events_round_trip_as_tagged_json() {
        let event = MouseEvent::Wheel {
            delta: -2,
            horizontal: false,
            time: 1.25,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<MouseEvent>(&json).unwrap(), event);
        assert!(json.contains(r#""type":"wheel""#));
    }
}
