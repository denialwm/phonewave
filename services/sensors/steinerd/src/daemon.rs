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

/// The data type that feeds each kind.
fn data_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Orientation => "accel",
        Kind::Light => "ambient_light",
        Kind::Proximity => "proximity",
    }
}

#[derive(Default)]
struct Sensor {
    uid: Option<Uid>,
    attrs: Option<Attributes>,
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
    sensors: BTreeMap<Kind, Sensor>,
    classifier: Classifier,
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
            state.props.has_orientation = false;
            state.props.has_light = false;
            state.props.has_proximity = false;
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
        for kind in Kind::ALL {
            core.request_suids(data_type(kind), true)
                .await
                .map_err(|e| format!("looking up {}: {e}", data_type(kind)))?;
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
        for kind in Kind::ALL {
            let claimed = self.claims.get(&kind).is_some_and(|c| !c.is_empty());
            let sensor = self.sensors.entry(kind).or_default();
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
                    eprintln!("steinerd: {verb} {}: {e}", kind.name());
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
        if let Some((data_type_name, uids)) = ssc::suid_answer(e) {
            let Some(kind) = Kind::ALL
                .into_iter()
                .find(|k| data_type(*k) == data_type_name)
            else {
                return;
            };
            let sensor = self.sensors.entry(kind).or_default();
            let uid = uids.first().copied();
            if sensor.uid == uid {
                return;
            }
            *sensor = Sensor {
                uid,
                ..Sensor::default()
            };
            self.set_has(kind, false);
            if let Some(uid) = uid
                && let Err(e) = core.request_attributes(uid).await
            {
                eprintln!("steinerd: {} attributes: {e}", kind.name());
            }
            return;
        }
        let Some((kind, sensor)) = self.sensors.iter_mut().find(|(_, s)| s.uid == Some(e.uid))
        else {
            return;
        };
        let kind = *kind;
        match (kind, e.msg_id) {
            (_, msg::ATTRIBUTES) => {
                let attrs = Attributes::decode(&e.payload);
                eprintln!(
                    "steinerd: {}: {} {}",
                    kind.name(),
                    attrs.str(attr::VENDOR).unwrap_or("?").trim(),
                    attrs.str(attr::NAME).unwrap_or("?").trim(),
                );
                let available = attrs.bool(attr::AVAILABLE) != Some(false);
                sensor.attrs = Some(attrs);
                self.set_has(kind, available);
            }
            (Kind::Orientation, msg::MEASUREMENT) => {
                // Packed or one field per axis.
                let floats: Vec<f32> = pb::Reader::new(&e.payload)
                    .filter(|(f, _)| *f == 1)
                    .flat_map(|(_, v)| v.floats())
                    .collect();
                if let [x, y, z, ..] = floats[..] {
                    let ns = (u128::from(e.timestamp) * 625 / 12) as u64;
                    if let Some(o) = self.classifier.feed(x, y, z, ns) {
                        self.props.orientation = o;
                    }
                }
            }
            (Kind::Light, msg::MEASUREMENT) => {
                if let Some(lux) =
                    pb::field(&e.payload, 1).and_then(|v| v.floats().first().copied())
                {
                    self.props.light = f64::from(lux);
                }
            }
            (Kind::Proximity, msg::PROXIMITY_EVENT) => {
                if let Some(near) = pb::field(&e.payload, 1).and_then(Value::as_u64) {
                    self.props.near = near != 0;
                }
            }
            _ => {}
        }
    }

    fn set_has(&mut self, kind: Kind, has: bool) {
        match kind {
            Kind::Orientation => self.props.has_orientation = has,
            Kind::Light => self.props.has_light = has,
            Kind::Proximity => self.props.has_proximity = has,
        }
    }
}
