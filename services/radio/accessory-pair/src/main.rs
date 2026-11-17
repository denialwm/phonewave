use accessory_pair::{
    ATTR_ADDRESS, ATTR_CAPACITY, ATTR_PEN_TYPE, Address, DockEvent, Step,
    agent::{self, Agent, Allowed},
    bluez, format_address_list, parse_address_list, pen_sensing, pen_type_change, plan,
};
use async_io::{Async, Timer};
use futures_lite::{FutureExt, StreamExt, future};
use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use zbus::{Connection, MatchRule, MessageStream, message::Type, zvariant::OwnedValue};

const DISCOVERY_TIME: Duration = Duration::from_secs(30);
const RETRY_AFTER: Duration = Duration::from_secs(20);
const RETRIES: u32 = 3;

/// The switch for the docked double-send test: set, the docked keyboard is
/// not blocked, so its Bluetooth link stays connected while its input
/// arrives over the dock.
fn no_block() -> bool {
    std::env::var_os("ACCESSORY_PAIR_NO_BLOCK").is_some()
}

enum Event {
    Dock(DockEvent, PathBuf),
    Battery(String, u8),
    /// A device's Bluetooth link came up or dropped.
    Connected(String),
    /// BlueZ (re)started or an adapter was powered: look at everything again.
    Bluez,
    Tick,
    Quit,
}

struct Accessory {
    kind: String,
    /// The address BlueZ knows it by; the dock's, unless that came reversed.
    bt: Address,
    docked: bool,
    /// Its input arrives over the dock while docked: keep it off Bluetooth then.
    wired_input: bool,
    /// The stylus: the touch controller senses it only while it is docked
    /// or connected.
    stylus: bool,
    /// The reporting device, while docked.
    syspath: Option<PathBuf>,
    attempts: u32,
}

struct Daemon {
    conn: Connection,
    allowed: Allowed,
    accessories: HashMap<Address, Accessory>,
    /// Accessories this service blocked, kept across restarts so that none
    /// stays blocked once undocked.
    blocked: BTreeSet<Address>,
    blocked_file: PathBuf,
    retries: Vec<(Instant, Address)>,
    /// The stylus, by the address BlueZ knows it by, kept across restarts
    /// so that its link is followed while it is away from the dock.
    pen: Option<Address>,
    pen_file: PathBuf,
}

fn log(msg: impl AsRef<str>) {
    eprintln!("accessory-pair: {}", msg.as_ref());
}

/// The dock driver registers the accessory's input devices under itself.
fn has_input_children(syspath: &Path) -> bool {
    fs::read_dir(syspath.join("input")).is_ok_and(|mut d| d.next().is_some())
}

/// A stylus is what its driver calls one or, found at start-up where the
/// event's type is not known, an accessory the dock charges (it takes the
/// accessory's battery level) without taking its input.
fn is_stylus(ev: &DockEvent, syspath: &Path, wired: bool) -> bool {
    ev.kind == "stylus" || (ev.present && !wired && syspath.join(ATTR_CAPACITY).exists())
}

/// Turns stylus sensing on or off on every touch controller that has it.
fn set_pen_sensing(sense: bool) {
    let Ok(mut e) = udev::Enumerator::new() else {
        return;
    };
    if e.match_attribute(ATTR_PEN_TYPE, "*").is_err() {
        return;
    }
    let Ok(devices) = e.scan_devices() else {
        return;
    };
    for device in devices {
        let attr = device.syspath().join(ATTR_PEN_TYPE);
        let Some(current) = fs::read_to_string(&attr)
            .ok()
            .and_then(|t| t.trim().parse::<u8>().ok())
        else {
            continue;
        };
        let Some(value) = pen_type_change(current, sense) else {
            continue;
        };
        match fs::write(&attr, format!("{value}\n")) {
            Ok(()) => log(format!(
                "stylus sensing {} -> {} = {value}",
                if sense { "on" } else { "off" },
                attr.display()
            )),
            Err(e) => log(format!("cannot write {}: {e}", attr.display())),
        }
    }
}

impl Daemon {
    fn save_blocked(&self) {
        let text = format_address_list(&self.blocked);
        let tmp = self.blocked_file.with_extension("new");
        if let Err(e) = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &self.blocked_file)) {
            log(format!("cannot save {}: {e}", self.blocked_file.display()));
        }
    }

    fn save_pen(&self) {
        let text = format_address_list(&self.pen);
        let tmp = self.pen_file.with_extension("new");
        if let Err(e) = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &self.pen_file)) {
            log(format!("cannot save {}: {e}", self.pen_file.display()));
        }
    }

    /// Senses the stylus while it is docked or its Bluetooth link is up.
    async fn update_pen(&self) {
        let Some(pen) = self.pen else {
            return;
        };
        let docked = self.accessories.values().any(|a| a.stylus && a.docked);
        let connected = match bluez::objects(&self.conn).await {
            Ok(objects) => bluez::connected(&objects, pen),
            Err(e) => {
                log(format!("stylus {pen}: cannot ask BlueZ: {e}"));
                false
            }
        };
        set_pen_sensing(pen_sensing(docked, connected));
    }

    async fn connected(&self, device: &str) {
        let Some(bt) = Address::from_object_path(device) else {
            return;
        };
        if self
            .pen
            .is_some_and(|pen| pen == bt || pen.reversed() == bt)
        {
            self.update_pen().await;
        }
    }

    async fn dock(&mut self, ev: DockEvent, syspath: PathBuf) {
        log(format!(
            "{} {} {}",
            ev.kind,
            ev.address,
            if ev.present { "docked" } else { "undocked" }
        ));
        let wired = ev.present && has_input_children(&syspath);
        let stylus = is_stylus(&ev, &syspath, wired);
        let acc = self.accessories.entry(ev.address).or_insert(Accessory {
            kind: ev.kind.clone(),
            bt: ev.address,
            docked: false,
            wired_input: false,
            stylus: false,
            syspath: None,
            attempts: 0,
        });
        acc.stylus |= stylus;
        acc.kind = ev.kind;
        acc.docked = ev.present;
        if ev.present {
            acc.wired_input = wired;
            acc.syspath = Some(syspath);
        } else {
            acc.syspath = None;
        }
        acc.attempts = 0;
        self.allowed.allow(ev.address);
        self.allowed.allow(ev.address.reversed());
        self.retries.retain(|(_, a)| *a != ev.address);
        self.reconcile(ev.address).await;
        if let Some(acc) = self.accessories.get(&ev.address).filter(|a| a.stylus) {
            if self.pen != Some(acc.bt) {
                self.pen = Some(acc.bt);
                self.save_pen();
            }
            self.update_pen().await;
        }
    }

    async fn reconcile(&mut self, key: Address) {
        if let Err(e) = self.try_reconcile(key).await {
            let Some(acc) = self.accessories.get_mut(&key) else {
                return;
            };
            log(format!("{} {}: {e}", acc.kind, acc.bt));
            if (acc.docked || acc.wired_input) && acc.attempts < RETRIES {
                acc.attempts += 1;
                log(format!(
                    "{} {}: trying again in {} s",
                    acc.kind,
                    acc.bt,
                    RETRY_AFTER.as_secs()
                ));
                self.retries.push((Instant::now() + RETRY_AFTER, key));
            }
        }
    }

    async fn try_reconcile(&mut self, key: Address) -> Result<(), Box<dyn Error>> {
        let conn = self.conn.clone();
        let objects = bluez::objects(&conn).await?;
        let Some(adapter) = bluez::powered_adapter(&objects) else {
            log("no powered Bluetooth adapter; waiting for one");
            return Ok(());
        };
        let adapter = adapter.as_str().to_owned();
        let acc = self.accessories.get_mut(&key).ok_or("unknown accessory")?;
        let mut found = bluez::find_device(&objects, &adapter, acc.bt);
        if found.is_none() {
            // paired in an earlier session under the reversed address
            let rev = acc.bt.reversed();
            if let Some(f) = bluez::find_device(&objects, &adapter, rev).filter(|f| f.1.paired) {
                acc.bt = rev;
                found = Some(f);
            }
        }
        let view = found.as_ref().map(|f| f.1).unwrap_or_default();
        let mut path = found.map(|f| f.0.as_str().to_owned());
        let steps = plan(acc.docked, acc.wired_input && !no_block(), &view);
        let (kind, docked, syspath) = (acc.kind.clone(), acc.docked, acc.syspath.clone());
        let name = format!("{kind} {}", acc.bt);
        if !steps.is_empty() {
            log(format!("{name}: {view:?} -> {steps:?}"));
        }
        for step in steps {
            let device = || path.clone().ok_or("no device object");
            match step {
                Step::Discover => {
                    let bt = self.accessories[&key].bt;
                    let Some((p, seen)) =
                        bluez::discover(&conn, &adapter, bt, DISCOVERY_TIME).await?
                    else {
                        return Err(format!(
                            "not seen in {} s of LE scanning; is it advertising?",
                            DISCOVERY_TIME.as_secs()
                        )
                        .into());
                    };
                    if seen != bt {
                        log(format!(
                            "{name}: found under the byte-reversed address {seen}: \
                             the dock driver reports the address in the wrong order"
                        ));
                        self.accessories.get_mut(&key).unwrap().bt = seen;
                    }
                    log(format!("{name}: found as {p}"));
                    path = Some(p.as_str().to_owned());
                }
                Step::Pair => {
                    bluez::register_agent(&conn).await?;
                    bluez::pair(&conn, &device()?).await?;
                    log(format!("{name}: paired (Just Works, no prompt)"));
                }
                Step::Trust => bluez::set_flag(&conn, &device()?, "Trusted", true).await?,
                Step::Connect => match bluez::connect(&conn, &device()?).await {
                    Ok(()) => log(format!("{name}: connected")),
                    // an accessory that is not advertising connects by itself later
                    Err(e) => log(format!("{name}: not connected now: {e}")),
                },
                Step::Block => {
                    bluez::set_flag(&conn, &device()?, "Blocked", true).await?;
                    self.blocked.insert(self.accessories[&key].bt);
                    self.save_blocked();
                    log(format!(
                        "{name}: kept off Bluetooth while docked (its keys come over the dock)"
                    ));
                }
                Step::Unblock => {
                    bluez::set_flag(&conn, &device()?, "Blocked", false).await?;
                    self.blocked.remove(&self.accessories[&key].bt);
                    self.save_blocked();
                    log(format!("{name}: allowed on Bluetooth again"));
                }
            }
        }
        if let (true, Some(syspath), Some(device)) = (docked, syspath, path) {
            let objects = bluez::objects(&conn).await?;
            if let Some(level) = bluez::battery(&objects, &device) {
                write_capacity(&name, &syspath, level);
            }
        }
        Ok(())
    }

    fn battery(&self, device: &str, level: u8) {
        let Some(bt) = Address::from_object_path(device) else {
            return;
        };
        for acc in self.accessories.values() {
            if acc.bt == bt && acc.docked {
                if let Some(syspath) = &acc.syspath {
                    write_capacity(&format!("{} {}", acc.kind, acc.bt), syspath, level);
                }
            }
        }
    }

    async fn reconcile_all(&mut self) {
        let keys: Vec<Address> = self.accessories.keys().copied().collect();
        for key in keys {
            self.reconcile(key).await;
        }
    }

    async fn run(&mut self, events: async_channel::Receiver<Event>) {
        loop {
            let next = self.retries.iter().map(|r| r.0).min();
            let event = events
                .recv()
                .or(async {
                    match next {
                        Some(at) => Timer::at(at).await,
                        None => future::pending().await,
                    };
                    Ok(Event::Tick)
                })
                .await
                .unwrap_or(Event::Quit);
            match event {
                Event::Dock(ev, syspath) => self.dock(ev, syspath).await,
                Event::Battery(device, level) => self.battery(&device, level),
                Event::Connected(device) => self.connected(&device).await,
                Event::Bluez => {
                    self.reconcile_all().await;
                    self.update_pen().await;
                }
                Event::Tick => {
                    let now = Instant::now();
                    let (due, later): (Vec<_>, Vec<_>) =
                        self.retries.drain(..).partition(|r| r.0 <= now);
                    self.retries = later;
                    for (_, key) in due {
                        self.reconcile(key).await;
                    }
                }
                Event::Quit => return,
            }
        }
    }
}

/// Where a dock takes the accessory's battery level from (the stylus charger
/// uses it to know when to charge again).
fn write_capacity(name: &str, syspath: &Path, level: u8) {
    let attr = syspath.join(ATTR_CAPACITY);
    if !attr.exists() {
        return;
    }
    match fs::write(&attr, format!("{level}\n")) {
        Ok(()) => log(format!("{name}: battery {level}% -> {}", attr.display())),
        Err(e) => log(format!("{name}: cannot write {}: {e}", attr.display())),
    }
}

fn dock_event(device: &udev::Device) -> Option<DockEvent> {
    DockEvent::from_env(|k| device.property_value(k).and_then(|v| v.to_str()))
}

/// Accessories that were docked before the service started.
fn coldplug() -> Vec<(DockEvent, PathBuf)> {
    let mut found = Vec::new();
    let Ok(mut e) = udev::Enumerator::new() else {
        return found;
    };
    if e.match_attribute(ATTR_ADDRESS, "*").is_err() {
        return found;
    }
    let Ok(devices) = e.scan_devices() else {
        return found;
    };
    for device in devices {
        let syspath = device.syspath().to_owned();
        let Ok(text) = fs::read_to_string(syspath.join(ATTR_ADDRESS)) else {
            continue;
        };
        let Ok(address) = text.parse() else { continue };
        let kind = device
            .subsystem()
            .and_then(|s| s.to_str())
            .unwrap_or("accessory")
            .to_owned();
        found.push((
            DockEvent {
                address,
                present: true,
                kind,
            },
            syspath,
        ));
    }
    found
}

async fn watch_udev(tx: async_channel::Sender<Event>) -> Result<(), Box<dyn Error>> {
    let socket = Async::new(udev::MonitorBuilder::new()?.listen()?)?;
    loop {
        socket.readable().await?;
        for event in socket.get_ref().iter() {
            if event.event_type() != udev::EventType::Change {
                continue;
            }
            if let Some(ev) = dock_event(&event.device()) {
                tx.send(Event::Dock(ev, event.syspath().to_owned())).await?;
            }
        }
    }
}

async fn watch_bluez(
    conn: &Connection,
    tx: async_channel::Sender<Event>,
) -> Result<(), Box<dyn Error>> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(bluez::SERVICE)?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .build();
    let mut stream = MessageStream::for_match_rule(rule, conn, None).await?;
    while let Some(msg) = stream.next().await {
        let msg = msg?;
        let Some(path) = msg.header().path().map(|p| p.as_str().to_owned()) else {
            continue;
        };
        let Ok((iface, changed, _)) = msg
            .body()
            .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
        else {
            continue;
        };
        if iface == bluez::DEVICE && changed.contains_key("Connected") {
            tx.send(Event::Connected(path)).await?;
        } else if iface == bluez::BATTERY {
            if let Some(level) = changed.get("Percentage").and_then(|v| u8::try_from(v).ok()) {
                tx.send(Event::Battery(path, level)).await?;
            }
        } else if iface == bluez::ADAPTER
            && changed
                .get("Powered")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false)
        {
            tx.send(Event::Bluez).await?;
        }
    }
    Ok(())
}

/// A stylus docked before it connected: its battery shows up with the
/// Battery1 interface, after the dock event, and no PropertiesChanged
/// follows until the level moves.
async fn watch_bluez_added(
    conn: &Connection,
    tx: async_channel::Sender<Event>,
) -> Result<(), Box<dyn Error>> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(bluez::SERVICE)?
        .interface("org.freedesktop.DBus.ObjectManager")?
        .member("InterfacesAdded")?
        .build();
    let mut stream = MessageStream::for_match_rule(rule, conn, None).await?;
    while let Some(msg) = stream.next().await {
        let msg = msg?;
        let Ok((path, added)) = msg.body().deserialize::<(
            zbus::zvariant::OwnedObjectPath,
            HashMap<String, HashMap<String, OwnedValue>>,
        )>() else {
            continue;
        };
        let level = added
            .get(bluez::BATTERY)
            .and_then(|props| props.get("Percentage"))
            .and_then(|v| u8::try_from(v).ok());
        if let Some(level) = level {
            tx.send(Event::Battery(path.as_str().to_owned(), level))
                .await?;
        }
    }
    Ok(())
}

async fn watch_bluez_owner(
    conn: &Connection,
    tx: async_channel::Sender<Event>,
) -> Result<(), Box<dyn Error>> {
    let bus = zbus::fdo::DBusProxy::new(conn).await?;
    let mut owners = bus
        .receive_name_owner_changed_with_args(&[(0, bluez::SERVICE)])
        .await?;
    while let Some(change) = owners.next().await {
        if change.args()?.new_owner().is_some() {
            log("BlueZ started");
            tx.send(Event::Bluez).await?;
        }
    }
    Ok(())
}

async fn quit_on_signal(tx: async_channel::Sender<Event>) -> Result<(), Box<dyn Error>> {
    let mut signals =
        async_signal::Signals::new([async_signal::Signal::Term, async_signal::Signal::Int])?;
    signals.next().await;
    tx.send(Event::Quit).await?;
    future::pending().await
}

fn main() -> Result<(), Box<dyn Error>> {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" => println!("accessory-pair {}", env!("CARGO_PKG_VERSION")),
            "--help" => println!(
                "accessory-pair\nPairs the Bluetooth accessories that a dock driver reports \
                 (ACCESSORY_BT_ADDRESS uevents), silently."
            ),
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
        return Ok(());
    }
    let state = std::env::var_os("STATE_DIRECTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/var/lib/accessory-pair".into());
    let blocked_file = state.join("blocked");
    let pen_file = state.join("stylus");
    let pen = fs::read_to_string(&pen_file)
        .ok()
        .and_then(|t| parse_address_list(&t).into_iter().next());
    let blocked: BTreeSet<Address> = fs::read_to_string(&blocked_file)
        .map(|t| parse_address_list(&t).into_iter().collect())
        .unwrap_or_default();

    future::block_on(async {
        let allowed = Allowed::default();
        let conn = zbus::connection::Builder::system()?
            .serve_at(
                agent::PATH,
                Agent {
                    allowed: allowed.clone(),
                },
            )?
            .build()
            .await?;
        let (tx, rx) = async_channel::unbounded();
        // the udev monitor first, so no event between the scan and it is lost
        let udev_watch = watch_udev(tx.clone());
        let mut daemon = Daemon {
            conn: conn.clone(),
            allowed,
            accessories: HashMap::new(),
            blocked,
            blocked_file,
            retries: Vec::new(),
            pen,
            pen_file,
        };
        log("ready");
        let work = async {
            for (ev, syspath) in coldplug() {
                daemon.dock(ev, syspath).await;
            }
            // blocked in an earlier session and no longer docked: let it back
            let stale: Vec<Address> = daemon
                .blocked
                .iter()
                .copied()
                .filter(|b| !daemon.accessories.values().any(|a| a.bt == *b))
                .collect();
            for bt in stale {
                daemon.allowed.allow(bt);
                daemon.accessories.insert(
                    bt,
                    Accessory {
                        kind: "accessory".into(),
                        bt,
                        docked: false,
                        wired_input: true,
                        stylus: false,
                        syspath: None,
                        attempts: 0,
                    },
                );
                daemon.reconcile(bt).await;
            }
            // the stylus away from the dock since an earlier session
            daemon.update_pen().await;
            daemon.run(rx).await;
            Ok(())
        };
        udev_watch
            .or(watch_bluez(&conn, tx.clone()))
            .or(watch_bluez_added(&conn, tx.clone()))
            .or(watch_bluez_owner(&conn, tx.clone()))
            .or(quit_on_signal(tx))
            .or(work)
            .await
    })
}
