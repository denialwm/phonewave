//! Which edge of the screen points up, from the accelerometer.
//!
//! The accelerometer reads the reaction to gravity in the screen's frame: x
//! to the right, y to the top edge, z out of the glass. Upright in portrait
//! it reads about +9.8 on y.

/// The physical edge pointing up; `Normal` is the top edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Undefined,
    Normal,
    BottomUp,
    LeftUp,
    RightUp,
}

impl Orientation {
    pub fn name(self) -> &'static str {
        match self {
            Orientation::Undefined => "undefined",
            Orientation::Normal => "normal",
            Orientation::BottomUp => "bottom-up",
            Orientation::LeftUp => "left-up",
            Orientation::RightUp => "right-up",
        }
    }

    /// Where the gravity vector points in the screen plane, in degrees from
    /// the top edge, clockwise.
    fn angle(self) -> f32 {
        match self {
            Orientation::Normal | Orientation::Undefined => 0.0,
            Orientation::RightUp => 90.0,
            Orientation::BottomUp => 180.0,
            Orientation::LeftUp => 270.0,
        }
    }
}

/// Below this tilt from flat, in degrees, no edge points up.
const MIN_TILT: f32 = 25.0;
/// How far, in degrees, the screen must turn past the diagonal before it
/// counts as the next edge. Keeps it from flipping back and forth when held
/// near 45 degrees.
const HYSTERESIS: f32 = 15.0;
/// How long a new orientation must hold before it is reported: a turn of
/// the wrist on the way somewhere else is not a rotation.
const SETTLE_NS: u64 = 300_000_000;

pub struct Classifier {
    current: Orientation,
    candidate: Option<(Orientation, u64)>,
}

impl Default for Classifier {
    fn default() -> Self {
        Self {
            current: Orientation::Undefined,
            candidate: None,
        }
    }
}

impl Classifier {
    pub fn current(&self) -> Orientation {
        self.current
    }

    /// Takes one reading at `now_ns`; returns the new orientation when it
    /// changes.
    pub fn feed(&mut self, x: f32, y: f32, z: f32, now_ns: u64) -> Option<Orientation> {
        let Some(seen) = self.classify(x, y, z) else {
            self.candidate = None;
            return None;
        };
        if seen == self.current {
            self.candidate = None;
            return None;
        }
        match self.candidate {
            Some((c, since)) if c == seen => {
                if now_ns.saturating_sub(since) < SETTLE_NS {
                    return None;
                }
            }
            _ => {
                self.candidate = Some((seen, now_ns));
                // The first reading needs no settling: there is nothing to
                // keep.
                if self.current != Orientation::Undefined {
                    return None;
                }
            }
        }
        self.current = seen;
        self.candidate = None;
        Some(seen)
    }

    /// The edge pointing up in this reading, if the screen is tilted enough
    /// to have one. Near a diagonal, the current edge wins.
    fn classify(&self, x: f32, y: f32, z: f32) -> Option<Orientation> {
        let g = (x * x + y * y + z * z).sqrt();
        if g < 1.0 {
            return None;
        }
        let plane = x.hypot(y);
        if (plane / g).asin().to_degrees() < MIN_TILT {
            return None;
        }
        // Clockwise from the top edge: the edge the reading points to.
        let angle = x.atan2(y).to_degrees().rem_euclid(360.0);
        if self.current != Orientation::Undefined
            && distance(angle, self.current.angle()) <= 45.0 + HYSTERESIS
        {
            return Some(self.current);
        }
        Some(match ((angle + 45.0) / 90.0) as u32 % 4 {
            0 => Orientation::Normal,
            1 => Orientation::RightUp,
            2 => Orientation::BottomUp,
            _ => Orientation::LeftUp,
        })
    }
}

fn distance(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: f32 = 9.8;
    const MS: u64 = 1_000_000;

    #[test]
    fn names_the_edge_pointing_up() {
        let at = |x, y| {
            let mut c = Classifier::default();
            c.feed(x, y, 0.0, 0)
        };
        assert_eq!(at(0.0, G), Some(Orientation::Normal));
        assert_eq!(at(0.0, -G), Some(Orientation::BottomUp));
        // Left edge up: the reading points to the left, -x.
        assert_eq!(at(-G, 0.0), Some(Orientation::LeftUp));
        assert_eq!(at(G, 0.0), Some(Orientation::RightUp));
    }

    #[test]
    fn flat_keeps_the_last_orientation() {
        let mut c = Classifier::default();
        assert_eq!(c.feed(0.0, 0.0, G, 0), None);
        assert_eq!(c.current(), Orientation::Undefined);
        c.feed(0.0, G, 0.0, 0);
        assert_eq!(c.feed(0.3, 0.5, G, 1000 * MS), None);
        assert_eq!(c.current(), Orientation::Normal);
    }

    #[test]
    fn a_turn_must_settle() {
        let mut c = Classifier::default();
        c.feed(0.0, G, 0.0, 0);
        assert_eq!(c.feed(-G, 0.0, 0.0, 1000 * MS), None);
        assert_eq!(c.feed(-G, 0.0, 0.0, 1200 * MS), None);
        assert_eq!(c.feed(-G, 0.0, 0.0, 1300 * MS), Some(Orientation::LeftUp));
        // Back before settling: nothing reported.
        assert_eq!(c.feed(0.0, G, 0.0, 1400 * MS), None);
        assert_eq!(c.feed(-G, 0.0, 0.0, 1500 * MS), None);
        assert_eq!(c.current(), Orientation::LeftUp);
    }

    #[test]
    fn diagonals_keep_the_current_edge() {
        let mut c = Classifier::default();
        c.feed(0.0, G, 0.0, 0);
        // 50 degrees toward the right edge: still normal.
        let (s, k) = 50f32.to_radians().sin_cos();
        assert_eq!(c.feed(-G * s, G * k, 0.0, 0), None);
        assert_eq!(c.feed(-G * s, G * k, 0.0, 1000 * MS), None);
        assert_eq!(c.current(), Orientation::Normal);
        // 65 degrees: past the hysteresis.
        let (s, k) = 65f32.to_radians().sin_cos();
        c.feed(-G * s, G * k, 0.0, 2000 * MS);
        assert_eq!(
            c.feed(-G * s, G * k, 0.0, 2400 * MS),
            Some(Orientation::LeftUp)
        );
    }
}
