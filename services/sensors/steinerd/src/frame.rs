//! How the sensor core's axes sit against the screen.
//!
//! Everything steinerd offers is in the screen's frame: x to the right edge,
//! y to the top edge, z out of the glass. The sensor core's frame is the
//! vendor's choice; where its y is not the screen's top edge, the boot
//! image's tail says which of its axes is, in `steinerd/top`: `+y` (the
//! default), `+x`, `-y` or `-x`. Both frames share z.

use std::fs;
use std::path::Path;

/// The file a boot image carries in its tail, which the phone mounts as its
/// modules folder.
const TAIL_FILE: &str = "steinerd/top";

/// The sensor core's axis that points to the screen's top edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Frame {
    #[default]
    Y,
    X,
    NegY,
    NegX,
}

impl Frame {
    /// The frame the booted kernel's tail names; the screen's own without
    /// one.
    pub fn load() -> Self {
        let Ok(release) = fs::read_to_string("/proc/sys/kernel/osrelease") else {
            return Self::default();
        };
        let path = Path::new("/usr/lib/modules")
            .join(release.trim())
            .join(TAIL_FILE);
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        match Self::parse(text.trim()) {
            Some(frame) => frame,
            None => {
                eprintln!(
                    "steinerd: {}: {:?} is not +y, +x, -y or -x",
                    path.display(),
                    text.trim()
                );
                Self::default()
            }
        }
    }

    fn parse(axis: &str) -> Option<Self> {
        Some(match axis {
            "+y" => Self::Y,
            "+x" => Self::X,
            "-y" => Self::NegY,
            "-x" => Self::NegX,
            _ => return None,
        })
    }

    /// A vector of the sensor core in the screen's frame.
    pub fn vector(self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        match self {
            Self::Y => (x, y, z),
            Self::X => (-y, x, z),
            Self::NegY => (-x, -y, z),
            Self::NegX => (y, -x, z),
        }
    }

    /// The sensor core's attitude (x, y, z, w) as the screen's: the same
    /// turn, after the quarter turns about z from the screen's frame to the
    /// core's.
    pub fn attitude(self, x: f32, y: f32, z: f32, w: f32) -> (f32, f32, f32, f32) {
        let (s, c) = match self {
            Self::Y => return (x, y, z, w),
            Self::X => (
                -std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ),
            Self::NegY => (1.0, 0.0),
            Self::NegX => (
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ),
        };
        (x * c + y * s, y * c - x * s, w * s + z * c, w * c - z * s)
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn the_named_axis_becomes_the_top_edge() {
        assert_eq!(Frame::Y.vector(0.0, 9.8, 0.0), (0.0, 9.8, 0.0));
        assert_eq!(Frame::X.vector(9.8, 0.0, 0.0), (-0.0, 9.8, 0.0));
        assert_eq!(Frame::NegY.vector(0.0, -9.8, 0.0), (-0.0, 9.8, 0.0));
        assert_eq!(Frame::NegX.vector(-9.8, 0.0, 0.0), (0.0, 9.8, 0.0));
    }

    #[test]
    fn frames_stay_right_handed() {
        // The screen's right edge, where the core's y points away from it.
        assert_eq!(Frame::X.vector(0.0, -1.0, 0.0), (1.0, 0.0, 0.0));
        assert_eq!(Frame::NegX.vector(0.0, 1.0, 0.0), (1.0, -0.0, 0.0));
    }

    #[test]
    fn attitude_turns_with_the_frame() {
        // Lying flat with the core's x to the north: the screen's top edge
        // points north, so in its own frame the screen is not turned.
        let half = std::f32::consts::FRAC_1_SQRT_2;
        // The core's frame turned 90 degrees counterclockwise about up puts
        // its x on north.
        let (x, y, z, w) = Frame::X.attitude(0.0, 0.0, half, half);
        assert!(
            x.abs() < 1e-6 && y.abs() < 1e-6 && z.abs() < 1e-6,
            "{x} {y} {z}"
        );
        assert!((w - 1.0).abs() < 1e-6, "{w}");
    }

    #[test]
    fn only_the_four_axes_parse() {
        assert_eq!(Frame::parse("+x"), Some(Frame::X));
        assert_eq!(Frame::parse("x"), None);
    }
}
