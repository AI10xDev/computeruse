use crate::MouseEvent;
use serde::Serialize;
use std::error::Error;
use std::fmt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Short-horizon estimate of observed cursor pixels, not raw input deltas.
/// Observation timestamps must share a monotonic origin.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct CursorEstimate {
    /// Latest observed position in pixels.
    pub position: Point,
    /// Velocity in pixels/second along the x and y axes.
    pub velocity: [f64; 2],
    #[serde(skip)]
    observed_at: Duration,
}

impl CursorEstimate {
    /// Starts at an observed pixel position with zero velocity.
    pub fn new(position: Point, observed_at: Duration) -> Self {
        Self {
            position,
            velocity: [0.0, 0.0],
            observed_at,
        }
    }

    /// Updates position and velocity from pixel displacement and elapsed seconds.
    /// Timestamps must strictly increase from the same monotonic origin; invalid
    /// timestamps return an error without changing state. Gaps over 250 ms reset
    /// velocity to zero.
    pub fn observe(&mut self, position: Point, observed_at: Duration) -> anyhow::Result<()> {
        anyhow::ensure!(
            observed_at > self.observed_at,
            "cursor observation timestamps must strictly increase"
        );
        let elapsed = observed_at - self.observed_at;
        let velocity = if elapsed > Duration::from_millis(250) {
            [0.0, 0.0]
        } else {
            let seconds = elapsed.as_secs_f64();
            [
                (i64::from(position.x) - i64::from(self.position.x)) as f64 / seconds,
                (i64::from(position.y) - i64::from(self.position.y)) as f64 / seconds,
            ]
        };
        *self = Self {
            position,
            velocity,
            observed_at,
        };
        Ok(())
    }

    /// Extrapolates from the latest observation in pixels at constant velocity,
    /// capping the horizon at 100 ms. This is neither visual target tracking nor
    /// a physical motion model.
    pub fn predict(&self, horizon: Duration) -> [f64; 2] {
        let seconds = horizon.min(Duration::from_millis(100)).as_secs_f64();
        [
            f64::from(self.position.x) + self.velocity[0] * seconds,
            f64::from(self.position.y) + self.velocity[1] * seconds,
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Segment {
    pub start: Point,
    pub end: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Turn {
    Clockwise,
    Collinear,
    CounterClockwise,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MouseTrajectory {
    vertices: Vec<Point>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrajectoryError;

impl fmt::Display for TrajectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mouse trajectory exceeds the supported coordinate range")
    }
}

impl Error for TrajectoryError {}

impl MouseTrajectory {
    /// Builds a directed one-dimensional path from the recorded movement edges.
    pub fn from_events(origin: Point, events: &[MouseEvent]) -> Result<Self, TrajectoryError> {
        let mut vertices = vec![origin];
        let mut position = origin;
        for event in events {
            if let MouseEvent::Move { dx, dy, .. } = *event {
                position = Point {
                    x: position.x.checked_add(dx).ok_or(TrajectoryError)?,
                    y: position.y.checked_add(dy).ok_or(TrajectoryError)?,
                };
                vertices.push(position);
            }
        }
        Ok(Self { vertices })
    }

    pub fn vertices(&self) -> &[Point] {
        &self.vertices
    }

    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.vertices.windows(2).map(|points| Segment {
            start: points[0],
            end: points[1],
        })
    }

    /// Classifies each turn by the signed area of its three-point triangle.
    pub fn turns(&self) -> impl Iterator<Item = Turn> + '_ {
        self.vertices.windows(3).map(|points| {
            let ab_x = i64::from(points[1].x) - i64::from(points[0].x);
            let ab_y = i64::from(points[1].y) - i64::from(points[0].y);
            let bc_x = i64::from(points[2].x) - i64::from(points[1].x);
            let bc_y = i64::from(points[2].y) - i64::from(points[1].y);
            match (i128::from(ab_x) * i128::from(bc_y) - i128::from(ab_y) * i128::from(bc_x))
                .signum()
            {
                -1 => Turn::Clockwise,
                0 => Turn::Collinear,
                _ => Turn::CounterClockwise,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_estimate_uses_irregular_observation_intervals() {
        let mut estimate = CursorEstimate::new(Point { x: 10, y: 20 }, Duration::from_secs(42));
        assert_eq!(estimate.velocity, [0.0, 0.0]);
        assert_eq!(estimate.predict(Duration::from_millis(100)), [10.0, 20.0]);

        estimate
            .observe(Point { x: 18, y: 16 }, Duration::from_millis(42_125))
            .unwrap();
        assert_eq!(estimate.velocity, [64.0, -32.0]);

        estimate
            .observe(Point { x: 26, y: 12 }, Duration::from_millis(42_375))
            .unwrap();
        assert_eq!(estimate.position, Point { x: 26, y: 12 });
        assert_eq!(estimate.velocity, [32.0, -16.0]);
    }

    #[test]
    fn cursor_estimate_stops_on_stationary_observation() {
        let mut estimate = CursorEstimate::new(Point { x: 0, y: 0 }, Duration::ZERO);
        estimate
            .observe(Point { x: 8, y: -4 }, Duration::from_millis(125))
            .unwrap();
        assert_eq!(estimate.velocity, [64.0, -32.0]);

        estimate
            .observe(Point { x: 8, y: -4 }, Duration::from_millis(250))
            .unwrap();
        assert_eq!(estimate.velocity, [0.0, 0.0]);
        assert_eq!(estimate.predict(Duration::from_millis(100)), [8.0, -4.0]);
    }

    #[test]
    fn cursor_estimate_caps_prediction_horizon() {
        let mut estimate = CursorEstimate::new(Point { x: 0, y: 0 }, Duration::ZERO);
        estimate
            .observe(Point { x: 25, y: -50 }, Duration::from_millis(125))
            .unwrap();

        assert_eq!(estimate.predict(Duration::ZERO), [25.0, -50.0]);
        assert_eq!(estimate.predict(Duration::from_millis(50)), [35.0, -70.0]);
        assert_eq!(estimate.predict(Duration::from_millis(100)), [45.0, -90.0]);
        assert_eq!(estimate.predict(Duration::MAX), [45.0, -90.0]);
    }

    #[test]
    fn cursor_estimate_resets_after_stale_observation_gap() {
        let mut estimate = CursorEstimate::new(Point { x: 0, y: 0 }, Duration::ZERO);
        estimate
            .observe(Point { x: 1, y: 2 }, Duration::from_millis(250))
            .unwrap();
        assert_eq!(estimate.velocity, [4.0, 8.0]);

        estimate
            .observe(Point { x: 100, y: -100 }, Duration::from_millis(501))
            .unwrap();
        assert_eq!(estimate.velocity, [0.0, 0.0]);
        assert_eq!(
            estimate.predict(Duration::from_millis(100)),
            [100.0, -100.0]
        );

        estimate
            .observe(Point { x: 101, y: -98 }, Duration::from_millis(751))
            .unwrap();
        assert_eq!(estimate.velocity, [4.0, 8.0]);
    }

    #[test]
    fn cursor_estimate_rejects_invalid_timestamps_without_mutation() {
        let mut estimate = CursorEstimate::new(Point { x: 0, y: 0 }, Duration::from_secs(1));
        estimate
            .observe(Point { x: 8, y: -4 }, Duration::from_millis(1_125))
            .unwrap();
        let previous = estimate;

        for observed_at in [Duration::from_millis(1_125), Duration::from_secs(1)] {
            assert!(
                estimate
                    .observe(Point { x: 100, y: 200 }, observed_at)
                    .is_err()
            );
            assert_eq!(estimate.position, previous.position);
            assert_eq!(estimate.velocity, previous.velocity);
            assert_eq!(estimate.observed_at, previous.observed_at);
        }
    }

    #[test]
    fn cursor_estimate_handles_extreme_coordinates_without_overflow() {
        let mut estimate = CursorEstimate::new(
            Point {
                x: i32::MIN,
                y: i32::MAX,
            },
            Duration::ZERO,
        );
        estimate
            .observe(
                Point {
                    x: i32::MAX,
                    y: i32::MIN,
                },
                Duration::from_millis(250),
            )
            .unwrap();
        assert_eq!(estimate.velocity, [17_179_869_180.0, -17_179_869_180.0]);
    }

    #[test]
    fn cursor_estimate_serializes_without_timestamp() {
        let estimate = CursorEstimate::new(Point { x: 12, y: 34 }, Duration::from_secs(42));
        assert_eq!(
            serde_json::to_value(estimate).unwrap(),
            serde_json::json!({
                "position": { "x": 12, "y": 34 },
                "velocity": [0.0, 0.0]
            })
        );
    }

    #[test]
    fn trajectory_preserves_edges_and_classifies_turns() {
        let events = [
            MouseEvent::Move {
                dx: 10,
                dy: 0,
                time: 1.0,
            },
            MouseEvent::Move {
                dx: 0,
                dy: 5,
                time: 2.0,
            },
            MouseEvent::Move {
                dx: -2,
                dy: 0,
                time: 3.0,
            },
        ];
        let trajectory = MouseTrajectory::from_events(Point { x: 960, y: 540 }, &events).unwrap();

        assert_eq!(
            trajectory.vertices(),
            [
                Point { x: 960, y: 540 },
                Point { x: 970, y: 540 },
                Point { x: 970, y: 545 },
                Point { x: 968, y: 545 },
            ]
        );
        assert_eq!(
            trajectory.turns().collect::<Vec<_>>(),
            [Turn::CounterClockwise, Turn::CounterClockwise]
        );
    }

    #[test]
    fn trajectory_rejects_coordinate_overflow() {
        let event = MouseEvent::Move {
            dx: 1,
            dy: 0,
            time: 0.0,
        };
        assert!(MouseTrajectory::from_events(Point { x: i32::MAX, y: 0 }, &[event],).is_err());
    }
}
