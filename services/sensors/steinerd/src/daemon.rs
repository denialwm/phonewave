//! The daemon: follows the sensor core, runs the sensors someone claims,
//! and turns their readings into the bus properties.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use async_channel::Receiver;
use async_signal::Signals;
use futures_lite::{StreamExt, future};

use crate::bus::{Bus, Command, Kind, Props};
use crate::orientation::Classifier;
use crate::pb::{self, Value};
use crate::qmi;
use crate::ssc::{self, Attributes, Core, Event, Uid, attr, msg};

/// Where readings come from: one data type on the sensor core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Accel,
    Gyro,
    /// The gyroscope's bias, which the core tracks while the phone is still.
    GyroCal,
    /// The core's rotation vector, fused from the accelerometer, gyroscope
    /// and a magnetometer it calibrates itself.
    RotationVector,
    Light,
    Proximity,
}

impl Source {
    const ALL: [Source; 6] = [
        Source::Accel,
        Source::Gyro,
        Source::GyroCal,
        Source::RotationVector,
        Source::Light,
        Source::Proximity,
    ];

    fn data_type(self) -> &'static str {
        match self {
            Source::Accel => "accel",
            Source::Gyro => "gyro",
            Source::GyroCal => "gyro_cal",
            Source::RotationVector => "rotv",
            Source::Light => "ambient_light",
            Source::Proximity => "proximity",
        }
    }
}

/// The sources a claim runs. The first gives the readings and says whether
/// the phone has the sensor; the gyroscope's calibration comes along where
/// the core has one.
fn sources(kind: Kind) -> &'static [Source] {
    match kind {
        Kind::Orientation | Kind::Accel => &[Source::Accel],
        Kind::Gyro => &[Source::Gyro, Source::GyroCal],
        Kind::Compass => &[Source::RotationVector],
        Kind::Light => &[Source::Light],
        Kind::Proximity => &[Source::Proximity],
    }
}

/// The DSP clock's ticks per second.
const TICKS: u64 = 19_200_000;
/// The heading is published at most 20 times a second, from a rotation
/// vector that comes 100 times.
const HEADING_TICKS: u64 = TICKS / 20;
/// Acceleration and rotation rate are published at most 30 times a second:
/// the core streams the physical sensor's rate to every client, 100 Hz or
/// more while the rotation vector runs.
const MOTION_TICKS: u64 = TICKS / 30;

/// Whether a value last published at `at` may be published again at `now`,
/// `every` ticks apart; if so, `now` becomes its time.
fn due(at: &mut Option<u64>, now: u64, every: u64) -> bool {
    let due = at.is_none_or(|at| now.wrapping_sub(at) >= every);
    if due {
        *at = Some(now);
    }
    due
}

#[derive(Default)]
struct Sensor {
    uid: Option<Uid>,
    attrs: Option<Attributes>,
    /// Its attributes came and do not say it is unavailable.
    available: bool,
    on: bool,
}

impl Sensor {
    /// How to start it: `None` on change, else the lowest rate it offers.
    fn rate(&self) -> Option<f32> {
        let attrs = self.attrs.as_ref()?;
        if attrs.on_change() {
            return None;
        }
        attrs.rates().into_iter().reduce(f32::min)
    }
}

pub struct Daemon {
    pub bus: Bus,
    pub commands: Receiver<Command>,
    pub signals: Signals,
}

enum Wake {
    Core(Result<Core, qmi::Error>),
    Events(Result<Vec<Event>, qmi::Error>),
    Gone,
    Command(Command),
    Stop,
}

#[derive(Default)]
struct State {
    props: Props,
    claims: BTreeMap<Kind, BTreeSet<String>>,
    sensors: BTreeMap<Source, Sensor>,
    classifier: Classifier,
    /// The gyroscope's bias in rad/s, as the core last reported it.
    gyro_bias: [f32; 3],
    /// When each throttled value was last published, in DSP ticks.
    heading_at: Option<u64>,
    acceleration_at: Option<u64>,
    rotation_rate_at: Option<u64>,
}

/// How long one wait for the sensor core lasts before it is logged again.
const CORE_WAIT: Duration = Duration::from_secs(60);

impl Daemon {
    pub async fn run(mut self) -> Result<(), String> {
        let mut state = State::default();
        loop {
            let wake = future::or(
                async { Wake::Core(Core::connect(CORE_WAIT).await) },
                self.outside(),
            )
            .await;
            let core = match wake {
                Wake::Core(Ok(core)) => core,
                Wake::Core(Err(e)) => {
                    eprintln!("steinerd: waiting for the sensor core: {e}");
                    continue;
                }
                Wake::Command(c) => {
                    state.command(c);
                    continue;
                }
                Wake::Stop => return Ok(()),
                _ => continue,
            };
            eprintln!("steinerd: sensor core up");
            match self.follow(&core, &mut state).await {
                Ok(()) => {
                    state.stop(&core).await;
                    return Ok(());
                }
                Err(e) => eprintln!("steinerd: sensor core lost: {e}"),
            }
            state.sensors.clear();
            state.refresh_has();
            self.bus.publish(&state.props);
        }
    }

    /// A command or a stop signal.
    async fn outside(&mut self) -> Wake {
        future::or(
            async {
                match self.commands.recv().await {
                    Ok(c) => Wake::Command(c),
                    Err(_) => Wake::Stop,
                }
            },
            async {
                self.signals.next().await;
                Wake::Stop
            },
        )
        .await
    }

    /// Runs on one core until it goes away (an error) or steinerd stops.
    async fn follow(&mut self, core: &Core, state: &mut State) -> Result<(), String> {
        for source in Source::ALL {
            core.request_suids(source.data_type(), true)
                .await
                .map_err(|e| format!("looking up {}: {e}", source.data_type()))?;
        }
        loop {
            let core_wake = async {
                future::or(async { Wake::Events(core.events().await) }, async {
                    let _ = core.gone().await;
                    Wake::Gone
                })
                .await
            };
            match future::or(core_wake, self.outside()).await {
                Wake::Events(Ok(events)) => {
                    for e in &events {
                        state.event(core, e).await;
                    }
                }
                Wake::Events(Err(e)) => return Err(e.to_string()),
                Wake::Gone => return Err("it left the bus".into()),
                Wake::Command(c) => state.command(c),
                Wake::Stop => return Ok(()),
                Wake::Core(_) => {}
            }
            state.sync(core).await;
            self.bus.publish(&state.props);
        }
    }
}

impl State {
    fn command(&mut self, c: Command) {
        match c {
            Command::Claim(kind, who) => {
                self.claims.entry(kind).or_default().insert(who);
            }
            Command::Release(kind, who) => {
                self.claims.entry(kind).or_default().remove(&who);
            }
            Command::Gone(who) => {
                for holders in self.claims.values_mut() {
                    holders.remove(&who);
                }
            }
        }
    }

    /// Starts the claimed sensors that are ready and stops the rest.
    async fn sync(&mut self, core: &Core) {
        for source in Source::ALL {
            let claimed = Kind::ALL.into_iter().any(|kind| {
                sources(kind).contains(&source)
                    && self.claims.get(&kind).is_some_and(|c| !c.is_empty())
            });
            let sensor = self.sensors.entry(source).or_default();
            let Some(uid) = sensor.uid else { continue };
            let want = claimed && sensor.attrs.is_some();
            if want == sensor.on {
                continue;
            }
            let done = if want {
                core.enable(uid, sensor.rate()).await
            } else {
                core.disable(uid).await
            };
            match done {
                Ok(()) => sensor.on = want,
                Err(e) => {
                    let verb = if want { "starting" } else { "stopping" };
                    eprintln!("steinerd: {verb} {}: {e}", source.data_type());
                    // A failed stop still leaves no one listening.
                    sensor.on = false;
                }
            }
        }
    }

    async fn stop(&mut self, core: &Core) {
        self.claims.clear();
        self.sync(core).await;
    }

    async fn event(&mut self, core: &Core, e: &Event) {
        if let Some((data_type, uids)) = ssc::suid_answer(e) {
            let Some(source) = Source::ALL.into_iter().find(|s| s.data_type() == data_type) else {
                return;
            };
            let sensor = self.sensors.entry(source).or_default();
            let uid = uids.first().copied();
            if sensor.uid == uid {
                return;
            }
            *sensor = Sensor {
                uid,
                ..Sensor::default()
            };
            self.refresh_has();
            if let Some(uid) = uid
                && let Err(e) = core.request_attributes(uid).await
            {
                eprintln!("steinerd: {data_type} attributes: {e}");
            }
            return;
        }
        let Some((source, sensor)) = self.sensors.iter_mut().find(|(_, s)| s.uid == Some(e.uid))
        else {
            return;
        };
        let source = *source;
        // Packed or one field per value.
        let floats = || -> Vec<f32> {
            pb::Reader::new(&e.payload)
                .filter(|(f, _)| *f == 1)
                .flat_map(|(_, v)| v.floats())
                .collect()
        };
        match (source, e.msg_id) {
            (_, msg::ATTRIBUTES) => {
                let attrs = Attributes::decode(&e.payload);
                eprintln!(
                    "steinerd: {}: {} {}",
                    source.data_type(),
                    attrs.str(attr::VENDOR).unwrap_or("?"),
                    attrs.str(attr::NAME).unwrap_or("?"),
                );
                sensor.available = attrs.bool(attr::AVAILABLE) != Some(false);
                sensor.attrs = Some(attrs);
                self.refresh_has();
            }
            (Source::Accel, msg::MEASUREMENT) => {
                if let [x, y, z, ..] = floats()[..] {
                    let ns = (u128::from(e.timestamp) * 625 / 12) as u64;
                    if let Some(o) = self.classifier.feed(x, y, z, ns) {
                        self.props.orientation = o;
                    }
                    if due(&mut self.acceleration_at, e.timestamp, MOTION_TICKS) {
                        self.props.acceleration = (x.into(), y.into(), z.into());
                    }
                }
            }
            (Source::Gyro, msg::MEASUREMENT) => {
                if let [x, y, z, ..] = floats()[..]
                    && due(&mut self.rotation_rate_at, e.timestamp, MOTION_TICKS)
                {
                    let [bx, by, bz] = self.gyro_bias;
                    self.props.rotation_rate = ((x - bx).into(), (y - by).into(), (z - bz).into());
                }
            }
            (Source::GyroCal, msg::CALIBRATION) => {
                if let [x, y, z, ..] = floats()[..] {
                    self.gyro_bias = [x, y, z];
                }
            }
            (Source::RotationVector, msg::MEASUREMENT) => {
                let accuracy = pb::field(&e.payload, 2)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(3) as u8;
                let changed = accuracy != self.props.heading_accuracy;
                if let [x, y, z, w, ..] = floats()[..]
                    && (due(&mut self.heading_at, e.timestamp, HEADING_TICKS) || changed)
                {
                    self.props.heading = (heading(x, y, z, w) * 10.0).round() / 10.0 % 360.0;
                    self.props.heading_accuracy = accuracy;
                }
            }
            (Source::Light, msg::MEASUREMENT) => {
                if let Some(lux) =
                    pb::field(&e.payload, 1).and_then(|v| v.floats().first().copied())
                {
                    self.props.light = f64::from(lux);
                }
            }
            (Source::Proximity, msg::PROXIMITY_EVENT) => {
                if let Some(near) = pb::field(&e.payload, 1).and_then(Value::as_u64) {
                    self.props.near = near != 0;
                }
            }
            _ => {}
        }
    }

    /// Whether the phone has each sensor, from its first source.
    fn refresh_has(&mut self) {
        let has = |kind| {
            self.sensors
                .get(&sources(kind)[0])
                .is_some_and(|s| s.available)
        };
        self.props.has_orientation = has(Kind::Orientation);
        self.props.has_light = has(Kind::Light);
        self.props.has_proximity = has(Kind::Proximity);
        self.props.has_accel = has(Kind::Accel);
        self.props.has_gyro = has(Kind::Gyro);
        self.props.has_compass = has(Kind::Compass);
    }
}

/// The heading of a rotation vector (x, y, z, w; the device's frame in the
/// world's, east, north, up), in degrees clockwise from north: where the
/// top edge points while the screen faces more up or down than sideways,
/// else where the back points. Held as phones are, past 45 degrees from
/// flat, both point the same way, so the switch does not jump.
fn heading(x: f32, y: f32, z: f32, w: f32) -> f64 {
    let (x, y, z, w) = (f64::from(x), f64::from(y), f64::from(z), f64::from(w));
    // Columns of the rotation matrix: the device's y and z axes in the
    // world's east, north and up.
    let top = (2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z));
    let back = (-2.0 * (x * z + y * w), -2.0 * (y * z - x * w));
    let screen_up = 1.0 - 2.0 * (x * x + y * y);
    let (east, north) = if screen_up.abs() >= std::f64::consts::FRAC_1_SQRT_2 {
        top
    } else {
        back
    };
    east.atan2(north).to_degrees().rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::heading;

    /// A turn of `degrees` about `axis` as a rotation vector.
    fn turn(axis: [f32; 3], degrees: f32) -> (f32, f32, f32, f32) {
        let (sin, cos) = (degrees.to_radians() / 2.0).sin_cos();
        (axis[0] * sin, axis[1] * sin, axis[2] * sin, cos)
    }

    fn close(a: f64, b: f64) -> bool {
        let d = (a - b).rem_euclid(360.0);
        d.min(360.0 - d) < 0.01
    }

    #[test]
    fn flat_heading_follows_the_top_edge() {
        let (x, y, z, w) = turn([0.0, 0.0, 1.0], 0.0);
        assert!(close(heading(x, y, z, w), 0.0));
        // Turned 90 degrees counterclockwise about up: the top edge points
        // west.
        let (x, y, z, w) = turn([0.0, 0.0, 1.0], 90.0);
        assert!(close(heading(x, y, z, w), 270.0));
    }

    #[test]
    fn upright_heading_follows_the_back() {
        // Stood up from flat, facing north: the screen faces south, the
        // back north.
        let (x, y, z, w) = turn([1.0, 0.0, 0.0], 90.0);
        assert!(close(heading(x, y, z, w), 0.0));
    }

    #[test]
    fn sample_from_the_redmagic() {
        // Lying on a desk, steinerctl raw rotv.
        let h = heading(0.0088, -0.0078, -0.938, 0.3465);
        assert!((h - 139.5).abs() < 1.0, "{h}");
    }
}
