use crate::MouseEvent;
use std::error::Error;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
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
