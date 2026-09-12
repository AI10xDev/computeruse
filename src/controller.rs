use crate::{Button, ButtonState, CursorEstimate, MouseEvent, Point};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::thread;
use std::time::{Duration, Instant};

const CLICK_HOLD_DURATION: Duration = Duration::from_millis(20);
const CURSOR_POLL_INTERVAL: Duration = Duration::from_millis(16);
const CURSOR_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CURSOR_DURATION: Duration = Duration::from_secs(10);
const CURSOR_TOLERANCE: f64 = 2.0;
const MAX_CURSOR_STEP: f64 = 96.0;

/// Observes and moves the cursor in the same desktop-pixel coordinate space.
/// Unlike relative input counts, these coordinates must not depend on acceleration.
pub trait CursorBackend {
    fn dimensions(&mut self) -> Result<(u16, u16)>;
    fn position(&mut self) -> Result<Point>;
    fn move_to(&mut self, position: Point) -> Result<()>;
}

/// Measured result of one normalized target-directed movement. Positions and
/// residuals are pixels; velocity is pixels/second, not relative input counts.
#[derive(Clone, Debug, Serialize)]
pub struct CursorMotion {
    pub start: Point,
    pub target: Point,
    pub estimate: CursorEstimate,
    pub corrections: usize,
    pub elapsed_ms: f64,
    pub residual_pixels: f64,
    pub peak_speed_pixels_per_second: f64,
}

/// Proportionally approaches a normalized 0..=65535 target, with bounded pixel
/// steps and short-horizon velocity damping. Requires two observations within
/// two pixels of the target. Stops after two seconds without progress or nominal
/// travel time plus ten seconds overall, allowing slow feedback while converging.
/// Backend calls must return promptly; this is not an I/O timeout. Errors prevent
/// the caller from proceeding to a dependent click.
pub fn move_cursor_to(cursor: &mut dyn CursorBackend, x: u16, y: u16) -> Result<CursorMotion> {
    let started = Instant::now();
    move_cursor_to_with_clock(cursor, x, y, |delay| {
        thread::sleep(delay);
        started.elapsed()
    })
}

fn move_cursor_to_with_clock(
    cursor: &mut dyn CursorBackend,
    x: u16,
    y: u16,
    mut clock: impl FnMut(Duration) -> Duration,
) -> Result<CursorMotion> {
    let (width, height) = cursor.dimensions()?;
    ensure!(
        width > 0 && height > 0,
        "cursor desktop dimensions must be nonzero"
    );
    let target = Point {
        x: ((u32::from(x) * u32::from(width - 1) + 32_767) / 65_535) as i32,
        y: ((u32::from(y) * u32::from(height - 1) + 32_767) / 65_535) as i32,
    };
    let start = cursor.position()?;
    let started = clock(Duration::ZERO);
    let distance =
        (f64::from(target.x) - f64::from(start.x)).hypot(f64::from(target.y) - f64::from(start.y));
    let budget =
        MAX_CURSOR_DURATION + CURSOR_POLL_INTERVAL * (distance / MAX_CURSOR_STEP).ceil() as u32;
    let mut now = started;
    let mut last_progress = started;
    let mut best_residual = distance;
    let mut estimate = CursorEstimate::new(start, now);
    let mut corrections = 0;
    let mut settled = 0;
    let mut peak_speed: f64 = 0.0;

    loop {
        let position = estimate.position;
        ensure!(
            (0..i32::from(width)).contains(&position.x)
                && (0..i32::from(height)).contains(&position.y),
            "observed cursor ({}, {}) is outside the {width}x{height} desktop",
            position.x,
            position.y
        );
        let error = [
            f64::from(target.x - position.x),
            f64::from(target.y - position.y),
        ];
        let residual = error[0].hypot(error[1]);
        if residual <= CURSOR_TOLERANCE {
            settled += 1;
            if settled == 2 {
                return Ok(CursorMotion {
                    start,
                    target,
                    estimate,
                    corrections,
                    elapsed_ms: (now - started).as_secs_f64() * 1000.0,
                    residual_pixels: residual,
                    peak_speed_pixels_per_second: peak_speed,
                });
            }
        } else {
            settled = 0;
        }

        // Only a new best distance renews the stall timer; jitter must not keep
        // a confined cursor alive indefinitely. Accept confirmed arrival first.
        if best_residual - residual >= 1.0 {
            best_residual = residual;
            last_progress = now;
        }
        ensure!(
            now - started < budget && now - last_progress < CURSOR_TIMEOUT,
            "cursor did not settle at ({}, {}) after {} ms and {corrections} corrections: observed ({}, {}), residual {residual:.2} pixels; no progress for {} ms (stall limit {} ms, total limit {} ms); remaining actions were not executed",
            target.x,
            target.y,
            (now - started).as_millis(),
            position.x,
            position.y,
            (now - last_progress).as_millis(),
            CURSOR_TIMEOUT.as_millis(),
            budget.as_millis()
        );

        if residual > CURSOR_TOLERANCE {
            let predicted = estimate.predict(CURSOR_POLL_INTERVAL);
            // P control with velocity damping. Never command past the target or
            // reverse away from it merely because the short prediction overshoots.
            let mut step = [0.0; 2];
            for axis in 0..2 {
                let current = f64::from(if axis == 0 { position.x } else { position.y });
                step[axis] = (0.5 * error[axis] - 0.15 * (predicted[axis] - current))
                    .clamp(error[axis].min(0.0), error[axis].max(0.0));
            }
            let scale = (MAX_CURSOR_STEP / step[0].hypot(step[1])).min(1.0);
            let next = Point {
                x: position.x + (step[0] * scale) as i32,
                y: position.y + (step[1] * scale) as i32,
            };
            if next != position {
                cursor.move_to(next)?;
                corrections += 1;
            }
        }

        clock(CURSOR_POLL_INTERVAL);
        let position = cursor.position()?;
        now = clock(Duration::ZERO);
        estimate.observe(position, now)?;
        peak_speed = peak_speed.max(estimate.velocity[0].hypot(estimate.velocity[1]));
    }
}

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
        thread::sleep(CLICK_HOLD_DURATION);
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
        let mut button_down_at = [None; 5];
        for event in events {
            if speed > 0.0
                && let Some(previous) = previous_time
            {
                let delay = ((event.timestamp() - previous) / speed).max(0.0);
                thread::sleep(Duration::from_secs_f64(delay));
            }
            previous_time = Some(event.timestamp());

            match *event {
                MouseEvent::Button { button, state, .. } if filter.buttons => match state {
                    ButtonState::Down => {
                        self.backend.button(button, state)?;
                        button_down_at[button_index(button)] = Some(Instant::now());
                    }
                    ButtonState::Up => {
                        let index = button_index(button);
                        if let Some(pressed_at) = button_down_at[index] {
                            thread::sleep(CLICK_HOLD_DURATION.saturating_sub(pressed_at.elapsed()));
                        }
                        self.backend.button(button, state)?;
                        button_down_at[index] = None;
                    }
                },
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

fn button_index(button: Button) -> usize {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::Side => 3,
        Button::Extra => 4,
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
    use std::collections::VecDeque;
    use std::convert::Infallible;

    struct FakeCursor {
        position: Point,
        size: (u16, u16),
        response: f64,
        observations: VecDeque<Point>,
        moves: Vec<(Point, Point)>,
    }

    impl FakeCursor {
        fn new(position: Point, size: (u16, u16)) -> Self {
            Self {
                position,
                size,
                response: 1.0,
                observations: VecDeque::new(),
                moves: Vec::new(),
            }
        }
    }

    impl CursorBackend for FakeCursor {
        fn dimensions(&mut self) -> Result<(u16, u16)> {
            Ok(self.size)
        }

        fn position(&mut self) -> Result<Point> {
            if let Some(position) = self.observations.pop_front() {
                self.position = position;
            }
            Ok(self.position)
        }

        fn move_to(&mut self, target: Point) -> Result<()> {
            self.moves.push((self.position, target));
            self.position = Point {
                x: self.position.x
                    + (f64::from(target.x - self.position.x) * self.response).round() as i32,
                y: self.position.y
                    + (f64::from(target.y - self.position.y) * self.response).round() as i32,
            };
            Ok(())
        }
    }

    fn simulate(cursor: &mut FakeCursor, x: u16, y: u16) -> Result<CursorMotion> {
        let mut now = Duration::ZERO;
        move_cursor_to_with_clock(cursor, x, y, |delay| {
            now += delay;
            now
        })
    }

    #[test]
    fn proportional_steps_are_bounded_and_shrink_near_the_target() {
        let mut cursor = FakeCursor::new(Point { x: 0, y: 0 }, (1920, 1080));
        let report = simulate(&mut cursor, 32_768, 32_768).unwrap();

        assert_eq!(report.target, Point { x: 960, y: 540 });
        assert!(report.residual_pixels <= CURSOR_TOLERANCE);
        assert_eq!(report.estimate.position, cursor.position);
        assert_eq!(report.estimate.velocity, [0.0, 0.0]);
        assert!(report.peak_speed_pixels_per_second > 0.0);
        assert_eq!(report.corrections, cursor.moves.len());
        let lengths: Vec<_> = cursor
            .moves
            .iter()
            .map(|(from, to)| f64::from(to.x - from.x).hypot(f64::from(to.y - from.y)))
            .collect();
        assert!(lengths.len() > 2);
        assert!(lengths.iter().all(|length| *length <= MAX_CURSOR_STEP));
        assert!(lengths.last().unwrap() < lengths.first().unwrap());
    }

    #[test]
    fn normalized_targets_use_pixel_bounds_even_on_large_desktops() {
        for (size, input, target) in [
            ((1920, 1080), (0, 0), Point { x: 0, y: 0 }),
            ((1920, 1080), (65_535, 65_535), Point { x: 1919, y: 1079 }),
            ((3840, 2160), (32_768, 32_768), Point { x: 1920, y: 1080 }),
            ((11520, 2160), (65_535, 65_535), Point { x: 11519, y: 2159 }),
            ((1, 1), (65_535, 65_535), Point { x: 0, y: 0 }),
        ] {
            let mut cursor = FakeCursor::new(Point { x: 0, y: 0 }, size);
            let report = simulate(&mut cursor, input.0, input.1).unwrap();
            assert_eq!(report.target, target);
            assert!(report.residual_pixels <= CURSOR_TOLERANCE);
        }
    }

    #[test]
    fn controller_corrects_observed_undertravel_instead_of_trusting_commands() {
        let mut cursor = FakeCursor::new(Point { x: 50, y: 50 }, (1920, 1080));
        cursor.response = 0.5;
        let report = simulate(&mut cursor, 32_768, 32_768).unwrap();
        assert!(report.residual_pixels <= CURSOR_TOLERANCE);
        assert_eq!(report.estimate.position, cursor.position);
        assert!(report.corrections > 15);
    }

    #[test]
    fn delayed_and_overshooting_observations_require_consecutive_arrival_checks() {
        let mut cursor = FakeCursor::new(Point { x: 0, y: 0 }, (201, 1));
        cursor.observations = [
            Point { x: 0, y: 0 },
            Point { x: 0, y: 0 },
            Point { x: 110, y: 0 },
            Point { x: 100, y: 0 },
            Point { x: 105, y: 0 },
            Point { x: 100, y: 0 },
            Point { x: 100, y: 0 },
        ]
        .into();
        let report = simulate(&mut cursor, 32_768, 0).unwrap();

        assert!(cursor.observations.is_empty());
        assert_eq!(report.elapsed_ms, 96.0);
        assert_eq!(report.residual_pixels, 0.0);
        assert!(
            cursor
                .moves
                .iter()
                .any(|(from, to)| from.x == 110 && to.x < from.x)
        );
        assert!(
            cursor
                .moves
                .iter()
                .any(|(from, to)| from.x == 105 && to.x < from.x)
        );
    }

    #[test]
    fn already_arrived_cursor_is_observed_without_emitting_movement() {
        let mut cursor = FakeCursor::new(Point { x: 960, y: 540 }, (1920, 1080));
        let report = simulate(&mut cursor, 32_768, 32_768).unwrap();
        assert!(cursor.moves.is_empty());
        assert_eq!(report.elapsed_ms, 16.0);
    }

    #[test]
    fn stuck_cursor_exhausts_a_bounded_budget_without_claiming_arrival() {
        let mut cursor = FakeCursor::new(Point { x: 0, y: 0 }, (1920, 1080));
        cursor.response = 0.0;
        let error = simulate(&mut cursor, 32_768, 32_768).unwrap_err();
        assert!(error.to_string().contains("did not settle"));
        assert!(!cursor.moves.is_empty());
        assert!(cursor.moves.len() < 150);
    }

    #[test]
    fn slow_feedback_can_settle_while_making_progress() {
        let mut cursor = FakeCursor::new(Point { x: 300, y: 300 }, (1920, 1080));
        let mut now = Duration::ZERO;
        let report = move_cursor_to_with_clock(&mut cursor, 8_504, 790, |delay| {
            if !delay.is_zero() {
                now += Duration::from_millis(500);
            }
            now
        })
        .unwrap();

        assert_eq!(report.target, Point { x: 249, y: 13 });
        assert!(report.elapsed_ms > 2_064.0);
        assert!(report.residual_pixels <= CURSOR_TOLERANCE);
        assert_eq!(report.estimate.velocity, [0.0, 0.0]);
    }

    #[test]
    fn delayed_confirmation_accepts_observed_arrival() {
        let mut cursor = FakeCursor::new(Point { x: 960, y: 540 }, (1920, 1080));
        let mut now = Duration::ZERO;
        let report = move_cursor_to_with_clock(&mut cursor, 32_768, 32_768, |delay| {
            if !delay.is_zero() {
                now += Duration::from_secs(3);
            }
            now
        })
        .unwrap();

        assert!(cursor.moves.is_empty());
        assert_eq!(report.elapsed_ms, 3_000.0);
        assert_eq!(report.residual_pixels, 0.0);
    }

    #[test]
    fn progress_followed_by_a_stall_still_times_out() {
        let mut cursor = FakeCursor::new(Point { x: 300, y: 300 }, (1920, 1080));
        cursor.observations = [Point { x: 300, y: 300 }, Point { x: 255, y: 36 }].into();
        cursor.response = 0.0;
        let error = simulate(&mut cursor, 8_504, 790).unwrap_err().to_string();

        assert!(error.contains("did not settle"));
        assert!(error.contains("observed (255, 36), residual 23.77 pixels"));
        assert!(cursor.moves.len() < 150);
    }

    #[test]
    fn continuous_small_progress_has_a_hard_timeout() {
        let mut cursor = FakeCursor::new(Point { x: 0, y: 0 }, (1920, 1080));
        cursor.observations = (0..100).map(|x| Point { x, y: 0 }).collect();
        let mut now = Duration::ZERO;
        let error = move_cursor_to_with_clock(&mut cursor, 65_535, 0, |delay| {
            if !delay.is_zero() {
                now += Duration::from_millis(500);
            }
            now
        })
        .unwrap_err();

        assert!(error.to_string().contains("did not settle"));
        assert!(now >= Duration::from_secs(10));
        assert!(now < Duration::from_secs(11));
        assert!(!cursor.observations.is_empty());
    }

    #[test]
    fn invalid_cursor_geometry_or_observation_fails_before_movement() {
        for (position, size) in [
            (Point { x: 0, y: 0 }, (0, 1080)),
            (Point { x: 0, y: 0 }, (1920, 0)),
            (Point { x: -1, y: 0 }, (1920, 1080)),
            (Point { x: 1920, y: 0 }, (1920, 1080)),
        ] {
            let mut cursor = FakeCursor::new(position, size);
            assert!(simulate(&mut cursor, 32_768, 32_768).is_err());
            assert!(cursor.moves.is_empty());
        }
    }

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

    #[test]
    fn playback_preserves_a_minimum_button_hold() {
        let events = [
            MouseEvent::Button {
                button: Button::Left,
                state: ButtonState::Down,
                time: 1.0,
            },
            MouseEvent::Button {
                button: Button::Left,
                state: ButtonState::Up,
                time: 1.0,
            },
        ];
        let mut controller = Controller::new(FakeBackend::default());
        let started = Instant::now();

        controller
            .play(&events, 0.0, PlaybackFilter::default())
            .unwrap();

        assert!(started.elapsed() >= CLICK_HOLD_DURATION);
        assert_eq!(controller.backend.0, ["left:Down", "left:Up"]);
    }
}
