//! `org.steinerd.Sensors1` on `/org/steinerd`. zbus runs it on its own
//! thread; claims go to the daemon as a `Command`, and the daemon's readings
//! come back as property changes.

use async_channel::{Receiver, Sender};
use async_executor::LocalExecutor;
use futures_lite::StreamExt;
use zbus::message::Header;
use zbus::{Connection, interface};

use crate::orientation::Orientation;

pub const NAME: &str = "org.steinerd";
pub const PATH: &str = "/org/steinerd";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.steinerd.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    InvalidArgs(String),
    Failed(String),
}

/// What a client can claim. A claimed sensor streams; an unclaimed one is off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Orientation,
    Light,
    Proximity,
    Accel,
    Gyro,
    Compass,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Orientation,
        Kind::Light,
        Kind::Proximity,
        Kind::Accel,
        Kind::Gyro,
        Kind::Compass,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Orientation => "orientation",
            Kind::Light => "light",
            Kind::Proximity => "proximity",
            Kind::Accel => "accel",
            Kind::Gyro => "gyro",
            Kind::Compass => "compass",
        }
    }

    fn parse(name: &str) -> Result<Self, Error> {
        Kind::ALL
            .into_iter()
            .find(|k| k.name() == name)
            .ok_or_else(|| {
                Error::InvalidArgs(format!(
                    "no sensor {name:?}: orientation, light, proximity, accel, gyro or compass"
                ))
            })
    }
}

pub enum Command {
    Claim(Kind, String),
    Release(Kind, String),
    /// The client left the bus; its claims go with it.
    Gone(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Props {
    pub has_orientation: bool,
    pub orientation: Orientation,
    pub has_light: bool,
    pub light: f64,
    pub has_proximity: bool,
    pub near: bool,
    pub has_accel: bool,
    /// m/s², x to the right edge, y to the top edge, z out of the screen.
    pub acceleration: (f64, f64, f64),
    pub has_gyro: bool,
    /// rad/s about the same axes, counterclockwise looking down each.
    pub rotation_rate: (f64, f64, f64),
    pub has_compass: bool,
    /// Degrees clockwise from magnetic north; -1 before the first reading.
    pub heading: f64,
    /// 0 unreliable, 1 low, 2 medium, 3 high.
    pub heading_accuracy: u8,
}

impl Default for Props {
    fn default() -> Self {
        Self {
            has_orientation: false,
            orientation: Orientation::Undefined,
            has_light: false,
            light: 0.0,
            has_proximity: false,
            near: false,
            has_accel: false,
            acceleration: (0.0, 0.0, 0.0),
            has_gyro: false,
            rotation_rate: (0.0, 0.0, 0.0),
            has_compass: false,
            heading: -1.0,
            heading_accuracy: 0,
        }
    }
}

pub struct Sensors {
    props: Props,
    commands: Sender<Command>,
}

impl Sensors {
    async fn send(&self, command: Command) -> Result<(), Error> {
        self.commands
            .send(command)
            .await
            .map_err(|_| Error::Failed("steinerd is stopping".into()))
    }
}

fn sender(header: &Header<'_>) -> Result<String, Error> {
    header
        .sender()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::Failed("no sender".into()))
}

#[interface(name = "org.steinerd.Sensors1")]
impl Sensors {
    /// Starts a sensor for the caller: "orientation", "light",
    /// "proximity", "accel", "gyro" or "compass". It runs while anyone
    /// holds a claim; a claim ends with Release or when the caller leaves
    /// the bus.
    async fn claim(&self, sensor: &str, #[zbus(header)] header: Header<'_>) -> Result<(), Error> {
        let kind = Kind::parse(sensor)?;
        self.send(Command::Claim(kind, sender(&header)?)).await
    }

    async fn release(&self, sensor: &str, #[zbus(header)] header: Header<'_>) -> Result<(), Error> {
        let kind = Kind::parse(sensor)?;
        self.send(Command::Release(kind, sender(&header)?)).await
    }

    #[zbus(property)]
    fn has_orientation(&self) -> bool {
        self.props.has_orientation
    }

    /// The physical edge pointing up: normal (the top edge), bottom-up,
    /// left-up, right-up, or undefined before the first reading. Lying flat
    /// keeps the last one.
    #[zbus(property)]
    fn orientation(&self) -> &str {
        self.props.orientation.name()
    }

    #[zbus(property)]
    fn has_light(&self) -> bool {
        self.props.has_light
    }

    /// Ambient light in lux.
    #[zbus(property)]
    fn light(&self) -> f64 {
        self.props.light
    }

    #[zbus(property)]
    fn has_proximity(&self) -> bool {
        self.props.has_proximity
    }

    /// Something is close to the screen, like an ear on a call.
    #[zbus(property)]
    fn near(&self) -> bool {
        self.props.near
    }

    #[zbus(property)]
    fn has_accel(&self) -> bool {
        self.props.has_accel
    }

    /// The accelerometer in m/s²: x toward the right edge, y toward the top
    /// edge, z out of the screen. At rest it reads gravity's reaction, about
    /// +9.8 up.
    #[zbus(property)]
    fn acceleration(&self) -> (f64, f64, f64) {
        self.props.acceleration
    }

    #[zbus(property)]
    fn has_gyro(&self) -> bool {
        self.props.has_gyro
    }

    /// The gyroscope in rad/s about the accelerometer's axes, positive
    /// counterclockwise looking down each, its bias taken off.
    #[zbus(property)]
    fn rotation_rate(&self) -> (f64, f64, f64) {
        self.props.rotation_rate
    }

    #[zbus(property)]
    fn has_compass(&self) -> bool {
        self.props.has_compass
    }

    /// Degrees clockwise from magnetic north that the phone faces: its top
    /// edge while it lies nearer flat, its back once it stands nearer
    /// upright. -1 before the first reading.
    #[zbus(property)]
    fn heading(&self) -> f64 {
        self.props.heading
    }

    /// How far the heading can be trusted, as the magnetometer's
    /// calibration stands: 0 unreliable, 1 low, 2 medium, 3 high.
    #[zbus(property)]
    fn heading_accuracy(&self) -> u8 {
        self.props.heading_accuracy
    }
}

/// The daemon's end of the bus.
pub struct Bus {
    out: Sender<Props>,
}

impl Bus {
    pub async fn serve(ex: &LocalExecutor<'_>, commands: Sender<Command>) -> zbus::Result<Self> {
        let conn = zbus::connection::Builder::system()?
            .serve_at(
                PATH,
                Sensors {
                    props: Props::default(),
                    commands: commands.clone(),
                },
            )?
            .name(NAME)?
            .build()
            .await?;
        let (out, changes) = async_channel::unbounded();
        ex.spawn(publish(conn.clone(), changes)).detach();
        ex.spawn(follow_clients(conn, commands)).detach();
        Ok(Self { out })
    }

    /// Sets the properties, announcing the ones that changed.
    pub fn publish(&self, props: &Props) {
        // Closed only once steinerd stops.
        let _ = self.out.try_send(props.clone());
    }
}

/// Tells the daemon about every client that leaves the bus.
async fn follow_clients(conn: Connection, commands: Sender<Command>) {
    let Ok(bus) = zbus::fdo::DBusProxy::new(&conn).await else {
        return;
    };
    let Ok(mut owners) = bus.receive_name_owner_changed().await else {
        return;
    };
    while let Some(change) = owners.next().await {
        let Ok(args) = change.args() else { continue };
        if args.name().starts_with(':') && args.new_owner().is_none() {
            let _ = commands.send(Command::Gone(args.name().to_string())).await;
        }
    }
}

async fn publish(conn: Connection, changes: Receiver<Props>) {
    while let Ok(mut p) = changes.recv().await {
        // Only the newest matters.
        while let Ok(newer) = changes.try_recv() {
            p = newer;
        }
        if let Err(e) = apply(&conn, p).await {
            eprintln!("steinerd: publishing: {e}");
        }
    }
}

async fn apply(conn: &Connection, p: Props) -> zbus::Result<()> {
    let iface = conn.object_server().interface::<_, Sensors>(PATH).await?;
    let mut s = iface.get_mut().await;
    let old = std::mem::replace(&mut s.props, p.clone());
    let e = iface.signal_emitter();
    if old.has_orientation != p.has_orientation {
        s.has_orientation_changed(e).await?;
    }
    if old.orientation != p.orientation {
        s.orientation_changed(e).await?;
    }
    if old.has_light != p.has_light {
        s.has_light_changed(e).await?;
    }
    if old.light != p.light {
        s.light_changed(e).await?;
    }
    if old.has_proximity != p.has_proximity {
        s.has_proximity_changed(e).await?;
    }
    if old.near != p.near {
        s.near_changed(e).await?;
    }
    if old.has_accel != p.has_accel {
        s.has_accel_changed(e).await?;
    }
    if old.acceleration != p.acceleration {
        s.acceleration_changed(e).await?;
    }
    if old.has_gyro != p.has_gyro {
        s.has_gyro_changed(e).await?;
    }
    if old.rotation_rate != p.rotation_rate {
        s.rotation_rate_changed(e).await?;
    }
    if old.has_compass != p.has_compass {
        s.has_compass_changed(e).await?;
    }
    if old.heading != p.heading {
        s.heading_changed(e).await?;
    }
    if old.heading_accuracy != p.heading_accuracy {
        s.heading_accuracy_changed(e).await?;
    }
    Ok(())
}
