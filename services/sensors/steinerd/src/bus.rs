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
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Orientation, Kind::Light, Kind::Proximity];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Orientation => "orientation",
            Kind::Light => "light",
            Kind::Proximity => "proximity",
        }
    }

    fn parse(name: &str) -> Result<Self, Error> {
        Kind::ALL
            .into_iter()
            .find(|k| k.name() == name)
            .ok_or_else(|| {
                Error::InvalidArgs(format!(
                    "no sensor {name:?}: orientation, light or proximity"
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
    /// Starts a sensor for the caller: "orientation", "light" or
    /// "proximity". It runs while anyone holds a claim; a claim ends with
    /// Release or when the caller leaves the bus.
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
    Ok(())
}
