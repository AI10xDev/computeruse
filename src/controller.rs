use crate::{Button, ButtonState, MouseEvent};
use std::thread;
use std::time::Duration;

pub trait MouseBackend {
    type Error;

    fn button(&mut self, button: Button, state: ButtonState) -> Result<(), Self::Error>;
    fn move_relative(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error>;
    fn move_absolute(&mut self, x: u16, y: u16) -> Result<(), Self::Error>;
    fn wheel(&mut self, delta: i32, horizontal: bool) -> Result<(), Self::Error>;
}

pub struct Controller<B> {
    backend: B,
}

impl<B: MouseBackend> Controller<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn press(&mut self, button: Button) -> Result<(), B::Error> {
        self.backend.button(button, ButtonState::Down)
    }

    pub fn release(&mut self, button: Button) -> Result<(), B::Error> {
        self.backend.button(button, ButtonState::Up)
    }

    pub fn click(&mut self, button: Button) -> Result<(), B::Error> {
        self.press(button)?;
        self.release(button)
    }

    pub fn double_click(&mut self, button: Button) -> Result<(), B::Error> {
        self.click(button)?;
        self.click(button)
    }

    pub fn move_relative(&mut self, dx: i32, dy: i32) -> Result<(), B::Error> {
        self.backend.move_relative(dx, dy)
    }

    /// Moves to normalized desktop coordinates in the inclusive range 0..=65535.
    pub fn move_absolute(&mut self, x: u16, y: u16) -> Result<(), B::Error> {
        self.backend.move_absolute(x, y)
    }

    /// Moves to the normalized desktop origin at the upper-left corner.
    pub fn home(&mut self) -> Result<(), B::Error> {
        self.move_absolute(0, 0)
    }

    pub fn wheel(&mut self, delta: i32) -> Result<(), B::Error> {
        self.backend.wheel(delta, false)
    }

    pub fn horizontal_wheel(&mut self, delta: i32) -> Result<(), B::Error> {
        self.backend.wheel(delta, true)
    }

    pub fn drag_relative(&mut self, dx: i32, dy: i32, duration: Duration) -> Result<(), B::Error> {
        self.press(Button::Left)?;
        let move_result = self.move_relative_over(dx, dy, duration);
        let release_result = self.release(Button::Left);
        move_result.and(release_result)
    }

    pub fn move_relative_over(
        &mut self,
        dx: i32,
        dy: i32,
        duration: Duration,
    ) -> Result<(), B::Error> {
        let steps = ((duration.as_secs_f64() * 120.0).ceil() as u32).max(1);
        let pause = duration / steps;
        let mut previous_x = 0;
        let mut previous_y = 0;

        for step in 1..=steps {
            let x = ((dx as i64 * step as i64) / steps as i64) as i32;
            let y = ((dy as i64 * step as i64) / steps as i64) as i32;
            self.move_relative(x - previous_x, y - previous_y)?;
            previous_x = x;
            previous_y = y;
            if step != steps {
                thread::sleep(pause);
            }
        }
        Ok(())
    }

    pub fn play(
        &mut self,
        events: &[MouseEvent],
        speed: f64,
        filter: PlaybackFilter,
    ) -> Result<(), B::Error> {
        let mut previous_time: Option<f64> = None;
        for event in events {
            if speed > 0.0
                && let Some(previous) = previous_time
            {
                let delay = ((event.timestamp() - previous) / speed).max(0.0);
                thread::sleep(Duration::from_secs_f64(delay));
            }
            previous_time = Some(event.timestamp());

            match *event {
                MouseEvent::Button { button, state, .. } if filter.buttons => {
                    self.backend.button(button, state)?
                }
                MouseEvent::Move { dx, dy, .. } if filter.movement => {
                    self.backend.move_relative(dx, dy)?
                }
                MouseEvent::Wheel {
                    delta, horizontal, ..
                } if filter.wheel => self.backend.wheel(delta, horizontal)?,
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackFilter {
    pub buttons: bool,
    pub movement: bool,
    pub wheel: bool,
}

impl Default for PlaybackFilter {
    fn default() -> Self {
        Self {
            buttons: true,
            movement: true,
            wheel: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    #[derive(Default)]
    struct FakeBackend(Vec<String>);

    impl MouseBackend for FakeBackend {
        type Error = Infallible;

        fn button(&mut self, button: Button, state: ButtonState) -> Result<(), Self::Error> {
            self.0.push(format!("{button}:{state:?}"));
            Ok(())
        }
        fn move_relative(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
            self.0.push(format!("move:{dx},{dy}"));
            Ok(())
        }
        fn move_absolute(&mut self, x: u16, y: u16) -> Result<(), Self::Error> {
            self.0.push(format!("absolute:{x},{y}"));
            Ok(())
        }
        fn wheel(&mut self, delta: i32, horizontal: bool) -> Result<(), Self::Error> {
            self.0.push(format!("wheel:{delta},{horizontal}"));
            Ok(())
        }
    }

    #[test]
    fn click_and_double_click_preserve_order() {
        let mut controller = Controller::new(FakeBackend::default());
        controller.click(Button::Left).unwrap();
        controller.double_click(Button::Right).unwrap();
        assert_eq!(
            controller.backend.0,
            [
                "left:Down",
                "left:Up",
                "right:Down",
                "right:Up",
                "right:Down",
                "right:Up"
            ]
        );
    }

    #[test]
    fn animated_motion_reaches_exact_destination() {
        let mut controller = Controller::new(FakeBackend::default());
        controller
            .move_relative_over(7, -5, Duration::from_millis(25))
            .unwrap();
        let totals = controller.backend.0.iter().fold((0, 0), |(x, y), item| {
            let values = item.strip_prefix("move:").unwrap();
            let (dx, dy) = values.split_once(',').unwrap();
            (
                x + dx.parse::<i32>().unwrap(),
                y + dy.parse::<i32>().unwrap(),
            )
        });
        assert_eq!(totals, (7, -5));
    }

    #[test]
    fn home_moves_to_absolute_origin() {
        let mut controller = Controller::new(FakeBackend::default());
        controller.home().unwrap();
        assert_eq!(controller.backend.0, ["absolute:0,0"]);
    }

    #[test]
    fn playback_filter_excludes_requested_event_types() {
        let events = [
            MouseEvent::Button {
                button: Button::Left,
                state: ButtonState::Down,
                time: 1.0,
            },
            MouseEvent::Move {
                dx: 2,
                dy: 3,
                time: 1.0,
            },
            MouseEvent::Wheel {
                delta: 1,
                horizontal: false,
                time: 1.0,
            },
        ];
        let mut controller = Controller::new(FakeBackend::default());
        controller
            .play(
                &events,
                0.0,
                PlaybackFilter {
                    buttons: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(controller.backend.0, ["move:2,3", "wheel:1,false"]);
    }
}
