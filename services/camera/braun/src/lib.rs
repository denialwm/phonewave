//! braun: a phone's cameras, from the raw frames their sensors send to the
//! previews and photos apps show. The daemon finds the cameras in the media
//! graph, reads what the kernel says about each, adds the sensor's profile
//! and the device's calibration, and streams one camera at a time to the
//! client that opened it.

pub mod client;
pub mod proto;

#[cfg(feature = "service")]
pub mod bus;
#[cfg(feature = "service")]
pub mod camera;
#[cfg(feature = "service")]
pub mod device;
#[cfg(feature = "service")]
pub mod isp;
#[cfg(feature = "service")]
pub mod media;
#[cfg(feature = "service")]
pub mod photo;
#[cfg(feature = "service")]
pub mod profile;
#[cfg(feature = "service")]
pub mod session;
#[cfg(feature = "service")]
pub mod v4l2;

pub const NAME: &str = "org.braun";
pub const PATH: &str = "/org/braun";
pub const INTERFACE: &str = "org.braun.Camera1";
