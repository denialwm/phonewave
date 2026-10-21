//! The core: brings the modem up, follows it through its indications, holds
//! the data session and handles SMS. It runs on one thread; the D-Bus
//! interfaces reach it through `Command`s. See DESIGN.md, Boot.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use async_executor::{LocalExecutor, Task};
use async_io::Timer;
use async_signal::Signals;
use futures_lite::{StreamExt, future};

use crate::bus::{Bus, Command, DataProps, Error, ModemProps, Reply, now};
use crate::config::{self, Device, Settings};
use crate::netlink::{self, Netlink};
use crate::qmi::{self, Client, Message, service};
use crate::qrtr::{Addr, Event as QrtrEvent, Lookup, Service};
use crate::sim::{self, Pin};
use crate::sms::store::Store;
use crate::sms::{pdu, wms};
use crate::{apn, data, modem, network};

const SETUP: &str = "/usr/lib/dmail/modem-setup";
const SETUP_TIMEOUT: Duration = Duration::from_secs(60);
const SERVICES_TIMEOUT: Duration = Duration::from_secs(30);
const NEEDED: [u32; 7] = [
    service::DMS,
    service::UIM,
    service::NAS,
    service::WDS,
    service::WMS,
    service::DPM,
    service::WDA,
];
/// Right after its services appear the modem answers some requests with
/// "not ready" for a moment: dmail asks again this often, for this long.
const READY_POLL: Duration = Duration::from_millis(500);
const READY_WAIT: Duration = Duration::from_secs(20);
/// This many modem crashes within `CRASH_WINDOW` and dmail gives up.
const CRASH_LIMIT: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(600);
const LINK: &str = "rmnet_data0";
const MUX_ID: u8 = 1;
const ROUTE_METRIC: u32 = 10000;
const FIRST_RETRY: Duration = Duration::from_secs(2);
const LAST_RETRY: Duration = Duration::from_secs(60);
/// The modem's message format for GSM/UMTS point-to-point SMS.
const FORMAT_GW_PP: u8 = 6;
/// Reject causes after which the modem stops asking for packet service until
/// the radio cycles: 3GPP TS 24.301, 7, 8 and 14.
const PS_REFUSED: [u8; 3] = [7, 8, 14];

fn log(what: impl std::fmt::Display) {
    eprintln!("dmail: {what}");
}

fn qmi_error(what: &str) -> impl FnOnce(qmi::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

pub struct Daemon<'a> {
    pub ex: &'a LocalExecutor<'static>,
    pub bus: Bus,
    pub device: Device,
    pub store: Arc<Mutex<Store>>,
    pub state_dir: PathBuf,
    pub commands: Receiver<Command>,
    pub signals: Signals,
    pub settings: Settings,
}

enum Event {
    Uim(Message),
    Nas(Message),
    Wds(Message),
    Wms(Message),
    Command(Command),
    ModemGone,
    Retry,
    Stop,
}

/// How one life of the modem ended.
enum End {
    Stop,
    ModemGone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sim {
    Missing,
    Locked,
    Ready,
}

impl Daemon<'_> {
    /// Brings the modem up and follows it. A failure leaves State `failed`
    /// until the modem's services register anew, then dmail starts over.
    pub async fn run(mut self) -> Result<(), String> {
        let mut crashes = VecDeque::new();
        loop {
            let reason = match self.life().await {
                Ok(End::Stop) => return Ok(()),
                Ok(End::ModemGone) => {
                    log("the modem went away");
                    let now = Instant::now();
                    crashes.push_back(now);
                    crashes.retain(|&t| now.duration_since(t) < CRASH_WINDOW);
                    if crashes.len() < CRASH_LIMIT {
                        continue;
                    }
                    format!("the modem went away {CRASH_LIMIT} times in 10 minutes")
                }
                Err(e) => e,
            };
            log(format!("failed: {reason}"));
            let props = ModemProps {
                state: "failed".into(),
                ..ModemProps::starting(self.settings.sim_slot)
            };
            self.publish_modem(&props).await;
            self.publish_data("failed", "").await;
            if !self.idle().await {
                return Ok(());
            }
            log("the modem's services registered again: starting over");
            crashes.clear();
        }
    }

    /// After a failure: answers what it can until the modem's services
    /// register anew (true) or dmail is told to stop (false).
    async fn idle(&mut self) -> bool {
        enum Idle {
            Command(Command),
            Restarted,
            Stop,
        }
        let mut restarted = pin!(modem_restarted());
        loop {
            let commands = &self.commands;
            let signals = &mut self.signals;
            let next = future::or(
                future::or(
                    async { commands.recv().await.map_or(Idle::Stop, Idle::Command) },
                    async {
                        signals.next().await;
                        Idle::Stop
                    },
                ),
                async {
                    restarted.as_mut().await;
                    Idle::Restarted
                },
            );
            let command = match next.await {
                Idle::Command(command) => command,
                Idle::Restarted => return true,
                Idle::Stop => return false,
            };
            match command {
                Command::UnlockPin(_, reply) => {
                    let _ = reply.try_send(Err(Error::NotReady("the modem failed".into())));
                }
                Command::SetData(enabled, reply) => {
                    self.settings.data_enabled = enabled;
                    self.save(config::DATA_ENABLED, enabled.into());
                    let _ = reply.try_send(Ok(()));
                }
                Command::SetSimSlot(slot, reply) => {
                    self.set_sim_slot(slot);
                    let _ = reply.try_send(Ok(()));
                    let props = ModemProps {
                        state: "failed".into(),
                        ..ModemProps::starting(slot)
                    };
                    self.publish_modem(&props).await;
                }
                Command::SetDeliveryReports(enabled, reply) => {
                    self.set_delivery_reports(enabled, reply).await;
                }
                Command::Send(id) => fail_send(&self.bus, &self.store, id).await,
            }
        }
    }

    fn save(&self, name: &str, value: u8) {
        if let Err(e) = config::save(&self.state_dir, name, value) {
            log(format!("saving {name}: {e}"));
        }
    }

    fn set_sim_slot(&mut self, slot: u8) {
        self.settings.sim_slot = slot;
        self.save(config::SIM_SLOT, slot);
    }

    async fn set_delivery_reports(&mut self, enabled: bool, reply: Reply) {
        self.settings.delivery_reports = enabled;
        self.save(config::DELIVERY_REPORTS, enabled.into());
        let _ = reply.try_send(Ok(()));
        if let Err(e) = self.bus.publish_delivery_reports(enabled).await {
            log(format!("publishing Messages1: {e}"));
        }
    }

    async fn publish_data(&self, state: &str, interface: &str) {
        let props = DataProps {
            enabled: self.settings.data_enabled,
            state: state.into(),
            interface: interface.into(),
        };
        if let Err(e) = self.bus.publish_data(&props).await {
            log(format!("publishing Data1: {e}"));
        }
    }

    async fn publish_modem(&self, props: &ModemProps) {
        if let Err(e) = self.bus.publish_modem(props).await {
            log(format!("publishing Modem1: {e}"));
        }
    }

    /// One life of the modem, from bringing it up to it going away or dmail
    /// stopping.
    async fn life(&mut self) -> Result<End, String> {
        let mut props = ModemProps::starting(self.settings.sim_slot);
        self.publish_modem(&props).await;
        run_setup().await?;
        let (lookup, found) = wait_services().await?;
        let client = |id: u32| -> Result<Rc<Client>, String> {
            Client::new(found[&id])
                .map(Rc::new)
                .map_err(|e| format!("QMI client: {e}"))
        };
        let m = Modem {
            dms: client(service::DMS)?,
            uim: client(service::UIM)?,
            nas: client(service::NAS)?,
            wds: client(service::WDS)?,
            wms: client(service::WMS)?,
            dpm: client(service::DPM)?,
            wda: client(service::WDA)?,
        };
        // Watched from here on: the modem can go away while it comes up.
        let (tx, rx) = async_channel::unbounded();
        let watch = watch_services(self.ex, lookup, found, tx.clone());
        let gone = async {
            let _ = rx.recv().await;
            Ok(false)
        };
        let up = async {
            when_ready(|| configure(&m))
                .await
                .map(|()| true)
                .map_err(|(step, e)| format!("{step}: {e}"))
        };
        if !future::or(up, gone).await? {
            return Ok(End::ModemGone);
        }

        let tasks = vec![
            forward(self.ex, m.uim.clone(), Event::Uim, tx.clone()),
            forward(self.ex, m.nas.clone(), Event::Nas, tx.clone()),
            forward(self.ex, m.wds.clone(), Event::Wds, tx.clone()),
            forward(self.ex, m.wms.clone(), Event::Wms, tx),
            watch,
        ];
        let mut life = Life {
            m,
            netlink: Netlink::new().map_err(|e| format!("rtnetlink: {e}"))?,
            endpoint: self.device.endpoint()?,
            sim: Sim::Missing,
            attached: false,
            port_ready: false,
            apn: None,
            session: None,
            retry: FIRST_RETRY,
            retry_at: None,
            refused: false,
            _tasks: tasks,
        };

        // A SIM that cannot be opened yet is tried again on its next status
        // indication.
        if let Err(e) = life.open_sim(self, &mut props).await {
            log(e);
        }
        life.refresh_network(self, &mut props).await;
        if life.sim == Sim::Ready {
            life.import_stored(self).await;
        }
        life.connect(self).await;

        let end = loop {
            let commands = &self.commands;
            let signals = &mut self.signals;
            let retry_at = life.retry_at;
            let event = future::or(
                future::or(
                    async { rx.recv().await.unwrap_or(Event::ModemGone) },
                    async { commands.recv().await.map_or(Event::Stop, Event::Command) },
                ),
                future::or(
                    async {
                        signals.next().await;
                        Event::Stop
                    },
                    async {
                        match retry_at {
                            Some(at) => Timer::at(at).await,
                            None => future::pending::<Instant>().await,
                        };
                        Event::Retry
                    },
                ),
            );
            match event.await {
                Event::Uim(msg) => life.on_uim(self, &mut props, msg).await,
                Event::Nas(msg) => life.on_nas(self, &mut props, msg).await,
                Event::Wds(msg) => life.on_wds(self, msg).await,
                Event::Wms(msg) => life.on_wms(self, msg).await,
                Event::Command(c) => life.on_command(self, &mut props, c).await,
                Event::Retry => {
                    life.retry_at = None;
                    life.connect(self).await;
                }
                Event::ModemGone => break End::ModemGone,
                Event::Stop => break End::Stop,
            }
        };
        life.end_session().await;
        self.publish_data(
            if self.settings.data_enabled {
                "connecting"
            } else {
                "off"
            },
            "",
        )
        .await;
        Ok(end)
    }
}

/// The usim on the SIM in `slot` (1-based) among `cards`, if there is one.
fn slot_usim(cards: &[sim::Card], slot: u8) -> Option<sim::App> {
    cards
        .get(usize::from(slot).wrapping_sub(1))
        .filter(|card| card.state == sim::CARD_PRESENT)
        .and_then(|card| card.usim().cloned())
}

/// Runs `attempt` until the modem is ready for it. While the modem answers
/// that it is not ready yet, dmail asks again every `READY_POLL` for at most
/// `READY_WAIT`; any other error ends it. Errors carry the step that failed.
async fn when_ready<T, F>(mut attempt: impl FnMut() -> F) -> Result<T, (&'static str, qmi::Error)>
where
    F: Future<Output = Result<T, (&'static str, qmi::Error)>>,
{
    let start = Instant::now();
    let mut last = None;
    loop {
        match attempt().await {
            Err((step, e)) if e.not_ready() && start.elapsed() < READY_WAIT => {
                let seen = Some((step, e.to_string()));
                if seen != last {
                    log(format!("{step}: {e}, the modem is not ready yet"));
                    last = seen;
                }
                Timer::after(READY_POLL).await;
            }
            Ok(value) => {
                if let Some((step, _)) = last {
                    let waited = start.elapsed().as_secs_f32();
                    log(format!("{step}: ready after {waited:.1} s"));
                }
                return Ok(value);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Puts the modem online and asks for the indications dmail follows. Every
/// step is safe to repeat. Errors carry the step.
async fn configure(m: &Modem) -> Result<(), (&'static str, qmi::Error)> {
    let mode = modem::operating_mode(&m.dms)
        .await
        .map_err(|e| ("operating mode", e))?;
    if mode != modem::ONLINE {
        modem::set_online(&m.dms)
            .await
            .map_err(|e| ("going online", e))?;
    }
    // Until dmail runs IMS: see DESIGN.md, Boot, step 4.
    network::set_voice_preference(&m.nas, network::DATA_CENTRIC, network::VOICE_CS_PREFERRED)
        .await
        .map_err(|e| ("voice preference", e))?;
    network::report_changes(&m.nas)
        .await
        .map_err(|e| ("NAS indications", e))?;
    wms::report_new_messages(&m.wms)
        .await
        .map_err(|e| ("SMS events", e))?;
    sim::report_status(&m.uim)
        .await
        .map_err(|e| ("SIM events", e))
}

/// What the usim's PIN1 state means; None while it settles.
fn sim_state(usim: &sim::App) -> Option<Sim> {
    match usim.pin1 {
        Pin::NotInitialized => None,
        Pin::Required | Pin::Blocked | Pin::PermanentlyBlocked => Some(Sim::Locked),
        _ => Some(Sim::Ready),
    }
}

async fn fail_send(bus: &Bus, store: &Mutex<Store>, id: u64) {
    if let Err(e) = store.lock().unwrap().set_state(id, "failed") {
        log(format!("SMS {id}: {e}"));
    }
    let _ = bus.message_changed(id, "failed").await;
}

/// Runs the device's `modem-setup`, if it has one.
async fn run_setup() -> Result<(), String> {
    if !Path::new(SETUP).exists() {
        return Ok(());
    }
    let mut child = std::process::Command::new(SETUP)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("{SETUP}: {e}"))?;
    let pid = child.id() as libc::pid_t;
    let wait = blocking::unblock(move || child.wait());
    let timeout = async {
        Timer::after(SETUP_TIMEOUT).await;
        None
    };
    match future::or(async { Some(wait.await) }, timeout).await {
        Some(Ok(status)) if status.success() => Ok(()),
        Some(Ok(status)) => Err(format!("modem-setup: {status}")),
        Some(Err(e)) => Err(format!("modem-setup: {e}")),
        None => {
            // SAFETY: plain kill(2). The waiting thread still holds the child,
            // so the pid cannot have been reused.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            Err(format!(
                "modem-setup still running after {} s",
                SETUP_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Waits until every service dmail uses is up. Returns the lookup, which keeps
/// reporting, and the services found, the lowest instance of each.
async fn wait_services() -> Result<(Lookup, HashMap<u32, Addr>), String> {
    let lookup = Lookup::new(0)
        .await
        .map_err(|e| format!("QRTR lookup: {e}"))?;
    let deadline = Instant::now() + SERVICES_TIMEOUT;
    let mut found: HashMap<u32, Service> = HashMap::new();
    while found.len() < NEEDED.len() {
        let timeout = async {
            Timer::at(deadline).await;
            None
        };
        match future::or(async { Some(lookup.next().await) }, timeout).await {
            Some(Ok(QrtrEvent::Up(s))) if NEEDED.contains(&s.id) => {
                if found.get(&s.id).is_none_or(|old| s.instance < old.instance) {
                    found.insert(s.id, s);
                }
            }
            Some(Ok(QrtrEvent::Down(s))) => found.retain(|_, old| old.addr != s.addr),
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(format!("QRTR lookup: {e}")),
            None => {
                let missing: Vec<_> = NEEDED
                    .iter()
                    .filter(|id| !found.contains_key(id))
                    .map(|&id| service::name(id).unwrap_or("?"))
                    .collect();
                return Err(format!(
                    "modem services missing after {} s: {}",
                    SERVICES_TIMEOUT.as_secs(),
                    missing.join(", ")
                ));
            }
        }
    }
    let found = found.into_iter().map(|(id, s)| (id, s.addr)).collect();
    Ok((lookup, found))
}

/// Resolves when a service dmail uses registers after the first listing:
/// the modem started again, by kernel recovery or by hand. Never resolves if
/// the name service cannot be followed.
async fn modem_restarted() {
    let lookup = match Lookup::new(0).await {
        Ok(lookup) => lookup,
        Err(e) => {
            log(format!("QRTR lookup: {e}"));
            return future::pending().await;
        }
    };
    let mut listed = false;
    loop {
        match lookup.next().await {
            Ok(QrtrEvent::Listed) => listed = true,
            Ok(QrtrEvent::Up(s)) if listed && NEEDED.contains(&s.id) => return,
            Ok(_) => {}
            Err(e) => {
                log(format!("QRTR lookup: {e}"));
                return future::pending().await;
            }
        }
    }
}

/// Passes a client's indications on as events.
fn forward(
    ex: &LocalExecutor<'static>,
    client: Rc<Client>,
    wrap: fn(Message) -> Event,
    tx: Sender<Event>,
) -> Task<()> {
    ex.spawn(async move {
        loop {
            let event = match client.indication().await {
                Ok(msg) => wrap(msg),
                Err(e) => {
                    log(format!("QMI socket: {e}"));
                    Event::ModemGone
                }
            };
            let gone = matches!(event, Event::ModemGone);
            if tx.send(event).await.is_err() || gone {
                return;
            }
        }
    })
}

/// Reports the modem going away: any service dmail uses leaving.
fn watch_services(
    ex: &LocalExecutor<'static>,
    lookup: Lookup,
    found: HashMap<u32, Addr>,
    tx: Sender<Event>,
) -> Task<()> {
    ex.spawn(async move {
        loop {
            match lookup.next().await {
                Ok(QrtrEvent::Down(s)) if found.get(&s.id) == Some(&s.addr) => break,
                Ok(_) => {}
                Err(e) => {
                    log(format!("QRTR lookup: {e}"));
                    break;
                }
            }
        }
        let _ = tx.send(Event::ModemGone).await;
    })
}

struct Modem {
    dms: Rc<Client>,
    uim: Rc<Client>,
    nas: Rc<Client>,
    wds: Rc<Client>,
    wms: Rc<Client>,
    dpm: Rc<Client>,
    wda: Rc<Client>,
}

/// What dmail holds for one life of the modem.
struct Life {
    m: Modem,
    netlink: Netlink,
    endpoint: data::Endpoint,
    sim: Sim,
    /// Registered with packet service attached.
    attached: bool,
    /// The data port is open and framed; once per modem life.
    port_ready: bool,
    apn: Option<String>,
    /// The packet data handle of the running session.
    session: Option<u32>,
    retry: Duration,
    retry_at: Option<Instant>,
    /// The network refused packet service: no retries until it attaches again.
    refused: bool,
    _tasks: Vec<Task<()>>,
}

impl Life {
    /// Opens the primary provisioning session on the chosen slot's SIM and
    /// reads whether it wants a PIN. A session on another slot ends first, so
    /// SMS and data never go out on the SIM the user did not choose.
    async fn open_sim(&mut self, d: &Daemon<'_>, props: &mut ModemProps) -> Result<(), String> {
        let slot = d.settings.sim_slot;
        let status = sim::card_status(&self.m.uim)
            .await
            .map_err(qmi_error("SIM status"))?;
        if status.primary.is_some_and(|s| s != slot) {
            sim::deactivate(&self.m.uim, sim::SESSION_PRIMARY_GW)
                .await
                .map_err(qmi_error("ending the SIM session"))?;
        }
        let Some(mut usim) = slot_usim(&status.cards, slot) else {
            self.sim = Sim::Missing;
            return Ok(());
        };
        // A second SIM can run as subscription 2; it becomes the primary.
        if status.secondary == Some(slot) {
            sim::deactivate(&self.m.uim, sim::SESSION_SECONDARY_GW)
                .await
                .map_err(qmi_error("ending the second SIM session"))?;
        }
        sim::activate(&self.m.uim, slot, &usim.aid)
            .await
            .map_err(qmi_error("SIM session"))?;
        // The PIN state settles a moment after the session opens.
        for _ in 0..10 {
            if sim_state(&usim).is_some() {
                break;
            }
            Timer::after(Duration::from_secs(1)).await;
            usim = self.usim(slot).await?.unwrap_or(usim);
        }
        props.pin_retries = usim.pin1_retries.into();
        self.sim = sim_state(&usim).unwrap_or(Sim::Ready);
        Ok(())
    }

    /// The usim on the SIM in `slot` (1-based), if there is one.
    async fn usim(&self, slot: u8) -> Result<Option<sim::App>, String> {
        let status = sim::card_status(&self.m.uim)
            .await
            .map_err(qmi_error("SIM status"))?;
        Ok(slot_usim(&status.cards, slot))
    }

    /// Follows the modem's card status: a PIN entered elsewhere, a SIM taken
    /// out or put in.
    async fn on_uim(&mut self, d: &Daemon<'_>, props: &mut ModemProps, msg: Message) {
        if msg.id != sim::CARD_STATUS_IND {
            return;
        }
        let status = match sim::parse_status_indication(&msg) {
            Ok(status) => status,
            Err(e) => return log(format!("SIM status: {e}")),
        };
        let usim = slot_usim(&status.cards, d.settings.sim_slot);
        let was = self.sim;
        let now = match &usim {
            None => Sim::Missing,
            // A SIM just put in needs its session opened first.
            Some(_) if was == Sim::Missing => {
                if let Err(e) = self.open_sim(d, props).await {
                    log(e);
                }
                self.sim
            }
            Some(usim) => {
                props.pin_retries = usim.pin1_retries.into();
                sim_state(usim).unwrap_or(was)
            }
        };
        self.sim = now;
        if now == was {
            d.publish_modem(props).await;
            return;
        }
        if now == Sim::Ready {
            self.sim_ready(d, props).await;
        } else {
            self.end_session().await;
            self.refresh_network(d, props).await;
            self.connect(d).await;
        }
    }

    /// What follows the SIM becoming usable.
    async fn sim_ready(&mut self, d: &Daemon<'_>, props: &mut ModemProps) {
        self.refresh_network(d, props).await;
        self.import_stored(d).await;
        self.connect(d).await;
    }

    /// Reads registration, and the signal when the radio changed; publishes
    /// Modem1.
    async fn refresh_network(&mut self, d: &Daemon<'_>, props: &mut ModemProps) {
        match network::serving(&self.m.nas).await {
            Ok(s) => {
                let technology = s.technology().as_str();
                let radio_changed = technology != props.technology;
                props.technology = technology.into();
                props.operator = match &s.plmn {
                    Some((_, _, name)) if s.registered() => name.clone(),
                    _ => String::new(),
                };
                props.roaming = s.registered() && s.roaming.unwrap_or(false);
                props.state = match self.sim {
                    Sim::Missing => "no-sim",
                    Sim::Locked => "locked",
                    Sim::Ready if s.registered() => "registered",
                    Sim::Ready => "searching",
                }
                .into();
                let attached = s.registered() && s.packet_attached();
                if attached && !self.attached {
                    // Attached again: retry at once, with fresh backoff.
                    self.refused = false;
                    self.retry = FIRST_RETRY;
                    self.retry_at = Some(Instant::now());
                }
                self.attached = attached;
                if radio_changed {
                    props.signal = match network::signal(&self.m.nas).await {
                        Ok(signal) => signal.unwrap_or(0),
                        Err(e) => {
                            log(format!("signal: {e}"));
                            0
                        }
                    };
                }
            }
            Err(e) => log(format!("serving system: {e}")),
        }
        d.publish_modem(props).await;
    }

    async fn on_nas(&mut self, d: &Daemon<'_>, props: &mut ModemProps, msg: Message) {
        match msg.id {
            network::NETWORK_REJECT => {
                let Some(r) = network::parse_reject(&msg) else {
                    return;
                };
                log(format!(
                    "network reject: radio {} domain {} cause {} ({})",
                    r.radio,
                    r.domain,
                    r.cause,
                    r.cause_name()
                ));
                // See DESIGN.md, Open: the modem will not ask again until the
                // radio cycles.
                if r.domain >= 2 && PS_REFUSED.contains(&r.cause) {
                    self.refused = true;
                    self.retry_at = None;
                    if d.settings.data_enabled && self.session.is_none() {
                        d.publish_data("failed", "").await;
                    }
                }
            }
            network::SIGNAL_INFO_IND => {
                if let Some(signal) = network::parse_signal(&msg).filter(|&s| s != props.signal) {
                    props.signal = signal;
                    d.publish_modem(props).await;
                }
            }
            network::SERVING_SYSTEM | network::SYSTEM_INFO_IND => {
                self.refresh_network(d, props).await;
            }
            _ => {}
        }
    }

    async fn on_wds(&mut self, d: &Daemon<'_>, msg: Message) {
        if msg.id != data::WDS_PACKET_SERVICE_STATUS || self.session.is_none() {
            return;
        }
        if let Some((data::Status::Disconnected, why)) = data::parse_status(&msg) {
            log(format!("the network ended the data session: {why:?}"));
            self.session = None;
            self.drop_link();
            self.schedule_retry(d).await;
        }
    }

    async fn schedule_retry(&mut self, d: &Daemon<'_>) {
        if self.refused {
            d.publish_data("failed", "").await;
            return;
        }
        d.publish_data("connecting", "").await;
        self.retry_at = Some(Instant::now() + self.retry);
        self.retry = (self.retry * 2).min(LAST_RETRY);
    }

    /// Brings the session up if it is wanted and possible.
    async fn connect(&mut self, d: &Daemon<'_>) {
        if !d.settings.data_enabled {
            d.publish_data("off", "").await;
            return;
        }
        if self.session.is_some() {
            return;
        }
        if self.refused {
            d.publish_data("failed", "").await;
            return;
        }
        d.publish_data("connecting", "").await;
        if self.sim != Sim::Ready || !self.attached {
            return;
        }
        self.retry_at = None;
        match self.start_session(d).await {
            Ok(()) => {
                self.retry = FIRST_RETRY;
                d.publish_data("connected", LINK).await;
            }
            Err(e) => {
                log(format!("data session: {e}"));
                self.end_session().await;
                self.schedule_retry(d).await;
            }
        }
    }

    async fn start_session(&mut self, d: &Daemon<'_>) -> Result<(), String> {
        let cfg = &d.device.data;
        if !self.port_ready {
            data::open_port(&self.m.dpm, self.endpoint, cfg.dpm.rx, cfg.dpm.tx)
                .await
                .map_err(qmi_error("data port"))?;
            data::set_format(
                &self.m.wda,
                self.endpoint,
                cfg.dl_datagrams,
                cfg.dl_datagram_size,
            )
            .await
            .map_err(qmi_error("data format"))?;
            self.port_ready = true;
        }
        let apn = match &self.apn {
            Some(apn) => apn.clone(),
            None => {
                let apn = self.pick_apn().await;
                log(format!("APN {apn:?}"));
                self.apn.insert(apn).clone()
            }
        };

        let net = |e: std::io::Error| format!("{LINK}: {e}");
        // The link is dmail's alone: whatever is left of it is stale.
        self.drop_link();
        let flags = netlink::RMNET_INGRESS_DEAGGREGATION
            | netlink::RMNET_INGRESS_MAP_CKSUMV5
            | netlink::RMNET_EGRESS_MAP_CKSUMV5;
        self.netlink
            .add_rmnet(&cfg.ipa_netdev, LINK, MUX_ID.into(), flags)
            .map_err(net)?;
        self.netlink.set_up(&cfg.ipa_netdev, None).map_err(net)?;

        data::bind_mux(&self.m.wds, self.endpoint, MUX_ID)
            .await
            .map_err(qmi_error("binding the mux"))?;
        data::prefer_ipv4(&self.m.wds)
            .await
            .map_err(qmi_error("IP family"))?;
        let started = data::start(&self.m.wds, &apn)
            .await
            .map_err(qmi_error("starting"))?;
        self.session = Some(match started {
            (Ok(handle), _) => handle,
            (Err(e), why) => return Err(format!("refused: {e}, {why:?}")),
        });
        let s = data::settings(&self.m.wds)
            .await
            .map_err(qmi_error("settings"))?;
        let mtu = s.mtu.unwrap_or(cfg.mtu_max).min(cfg.mtu_max);
        self.netlink.set_up(LINK, Some(mtu)).map_err(net)?;
        self.netlink
            .add_address(LINK, s.address, s.prefix)
            .map_err(net)?;
        self.netlink
            .add_default_route(LINK, ROUTE_METRIC)
            .map_err(net)?;
        log(format!("data session up on {apn:?}, mtu {mtu}"));
        Ok(())
    }

    /// The device's override, else the provider database for the SIM's home
    /// network, else the modem's default.
    async fn pick_apn(&self) -> String {
        match config::apn_override(Path::new(config::APN_FILE)) {
            Ok(Some(apn)) => return apn,
            Ok(None) => {}
            Err(e) => log(e),
        }
        match network::home_network(&self.m.nas).await {
            Ok(Some((mcc, mnc))) => apn::lookup(mcc, mnc).unwrap_or_default(),
            Ok(None) => String::new(),
            Err(e) => {
                log(format!("home network: {e}"));
                String::new()
            }
        }
    }

    /// Stops the session, if there is one, and removes the link.
    async fn end_session(&mut self) {
        self.retry_at = None;
        if let Some(handle) = self.session.take()
            && let Err(e) = data::stop(&self.m.wds, handle).await
        {
            log(format!("stopping the data session: {e}"));
        }
        self.drop_link();
    }

    fn drop_link(&self) {
        if netlink::index(LINK).is_ok()
            && let Err(e) = self.netlink.delete_link(LINK)
        {
            log(format!("removing {LINK}: {e}"));
        }
    }

    /// Answers first: what follows an unlock or the toggle can take as long
    /// as starting a session.
    async fn on_command(&mut self, d: &mut Daemon<'_>, props: &mut ModemProps, command: Command) {
        match command {
            Command::UnlockPin(pin, reply) => {
                let result = self.unlock(d, props, pin).await;
                let unlocked = result.is_ok();
                let _ = reply.try_send(result);
                if unlocked {
                    self.sim_ready(d, props).await;
                }
            }
            Command::SetData(enabled, reply) => {
                d.settings.data_enabled = enabled;
                d.save(config::DATA_ENABLED, enabled.into());
                let _ = reply.try_send(Ok(()));
                if enabled {
                    self.refused = false;
                    self.retry = FIRST_RETRY;
                    self.connect(d).await;
                } else {
                    self.end_session().await;
                    d.publish_data("off", "").await;
                }
            }
            Command::SetSimSlot(slot, reply) => {
                let changed = slot != d.settings.sim_slot;
                d.set_sim_slot(slot);
                props.sim_slot = slot;
                let _ = reply.try_send(Ok(()));
                if changed {
                    self.switch_sim(d, props).await;
                } else {
                    d.publish_modem(props).await;
                }
            }
            Command::SetDeliveryReports(enabled, reply) => {
                d.set_delivery_reports(enabled, reply).await;
            }
            Command::Send(id) => {
                let report = d.settings.delivery_reports;
                let task = send(
                    d.bus.clone(),
                    d.store.clone(),
                    self.m.wms.clone(),
                    id,
                    report,
                );
                d.ex.spawn(task).detach();
            }
        }
    }

    /// Moves SMS and data to the SIM in the newly chosen slot.
    async fn switch_sim(&mut self, d: &Daemon<'_>, props: &mut ModemProps) {
        log(format!("moving to the SIM in slot {}", d.settings.sim_slot));
        self.end_session().await;
        self.apn = None;
        self.sim = Sim::Missing;
        if let Err(e) = self.open_sim(d, props).await {
            log(e);
        }
        if self.sim == Sim::Ready {
            self.sim_ready(d, props).await;
        } else {
            self.refresh_network(d, props).await;
            self.connect(d).await;
        }
    }

    /// One PIN attempt. The PIN goes no further than the modem.
    async fn unlock(
        &mut self,
        d: &Daemon<'_>,
        props: &mut ModemProps,
        pin: String,
    ) -> Result<(), Error> {
        if self.sim != Sim::Locked {
            return Err(Error::NotReady("the SIM is not locked".into()));
        }
        let verified = sim::verify_pin1(&self.m.uim, pin.as_bytes()).await;
        drop(pin);
        let (outcome, retries) = verified.map_err(|e| Error::Failed(e.to_string()))?;
        if let Some((left, _)) = retries {
            props.pin_retries = left.into();
        }
        match outcome {
            Ok(()) => {
                self.sim = Sim::Ready;
                Ok(())
            }
            Err(e) => {
                d.publish_modem(props).await;
                Err(Error::WrongPin(format!(
                    "{e}; {} tries left",
                    props.pin_retries
                )))
            }
        }
    }

    async fn on_wms(&mut self, d: &Daemon<'_>, msg: Message) {
        if msg.id != wms::EVENT_REPORT {
            return;
        }
        for incoming in wms::parse_event(&msg) {
            match incoming {
                wms::Incoming::Transferred {
                    ack,
                    transaction,
                    format,
                    tpdu,
                    on_ims,
                } => {
                    // Stored before the ack: if dmail dies in between, the
                    // network sends it again and the store ignores the copy.
                    if format == FORMAT_GW_PP {
                        receive(d, &tpdu, true).await;
                    } else {
                        log(format!("ignoring an SMS in format {format}"));
                    }
                    if ack && let Err(e) = wms::ack(&self.m.wms, transaction, on_ims).await {
                        log(format!("SMS ack: {e}"));
                    }
                }
                wms::Incoming::Stored { storage, index } => {
                    self.read_stored(d, storage, index, true).await;
                }
            }
        }
    }

    /// Reads one message the modem stored. It stays where it is: the store
    /// ignores a PDU it already has.
    async fn read_stored(&self, d: &Daemon<'_>, storage: u8, index: u32, announce: bool) {
        match wms::raw_read(&self.m.wms, storage, index).await {
            Ok((_, raw)) => match pdu::skip_smsc(&raw) {
                Ok(tpdu) => receive(d, tpdu, announce).await,
                Err(e) => log(format!("stored SMS {storage}/{index}: {e}")),
            },
            Err(e) => log(format!("reading stored SMS {storage}/{index}: {e}")),
        }
    }

    /// Takes in what the SIM and the modem stored while dmail was not
    /// listening, without announcing it.
    async fn import_stored(&self, d: &Daemon<'_>) {
        let client = &*self.m.wms;
        for storage in [0, 1] {
            // The SIM's storage answers a while after the SIM opens.
            let list = move || async move {
                wms::list(client, storage)
                    .await
                    .map_err(|e| ("listing stored SMS", e))
            };
            match when_ready(list).await {
                Ok(indexes) => {
                    for index in indexes {
                        self.read_stored(d, storage, index, false).await;
                    }
                }
                Err((step, e)) => log(format!("{step} in {storage}: {e}")),
            }
        }
    }
}

/// Decodes and stores one received TPDU: a message, announcing finished
/// ones, or a status report.
async fn receive(d: &Daemon<'_>, tpdu: &[u8], announce: bool) {
    let m = match pdu::decode(tpdu) {
        Ok(pdu::Tpdu::Deliver(m)) => m,
        Ok(pdu::Tpdu::StatusReport(r)) => return settle(d, &r).await,
        Err(e) => {
            log(format!("undecodable SMS: {e}"));
            return;
        }
    };
    let now = now();
    let done = {
        let mut store = d.store.lock().unwrap();
        store.receive(tpdu, &m, now).and_then(|done| {
            let stale = store.flush_stale_parts(now)?;
            Ok(done.into_iter().chain(stale).collect::<Vec<_>>())
        })
    };
    match done {
        Ok(messages) if announce => {
            for (id, peer) in messages {
                let _ = d.bus.message_received(id, &peer).await;
            }
        }
        Ok(_) => {}
        Err(e) => log(format!("storing an SMS: {e}")),
    }
}

/// Settles a sent message's state once every part has its report.
async fn settle(d: &Daemon<'_>, r: &pdu::StatusReport) {
    log(format!(
        "status report for reference {}: status {:#04x}",
        r.reference, r.status
    ));
    let Some(outcome) = r.outcome() else {
        return;
    };
    let settled = d
        .store
        .lock()
        .unwrap()
        .report(&r.recipient, r.reference, outcome);
    match settled {
        Ok(Some((id, state))) => {
            let _ = d.bus.message_changed(id, &state).await;
        }
        Ok(None) => {}
        Err(e) => log(format!("storing a status report: {e}")),
    }
}

/// Sends a stored outgoing message, part by part; with `report`, every part
/// asks for a status report.
async fn send(bus: Bus, store: Arc<Mutex<Store>>, wms: Rc<Client>, id: u64, report: bool) {
    let message = store.lock().unwrap().outgoing(id);
    let parts = match message {
        Ok(Some((peer, text))) => pdu::encode_submit(&peer, &text, (now() & 0xff) as u8, report)
            .map_err(|e| e.to_string()),
        Ok(None) => return,
        Err(e) => Err(e.to_string()),
    };
    let sent = match parts {
        Ok(parts) => send_parts(&store, &wms, id, &parts, report).await,
        Err(e) => Err(e),
    };
    let state = match sent {
        Ok(()) => "sent",
        Err(e) => {
            log(format!("SMS {id} not sent: {e}"));
            "failed"
        }
    };
    if let Err(e) = store.lock().unwrap().set_state(id, state) {
        log(format!("SMS {id}: {e}"));
    }
    let _ = bus.message_changed(id, state).await;
}

/// Sends the parts in order. A part asking for a report is recorded as soon
/// as it is out, before its report can arrive.
async fn send_parts(
    store: &Mutex<Store>,
    wms: &Client,
    id: u64,
    parts: &[Vec<u8>],
    report: bool,
) -> Result<(), String> {
    for part in parts {
        match wms::raw_send(wms, part).await {
            Ok((Ok(reference), _)) => {
                if report && let Err(e) = store.lock().unwrap().await_report(id, reference) {
                    log(format!("SMS {id}: {e}"));
                }
            }
            Ok((Err(e), cause)) => return Err(format!("{e}, cause {cause:?}")),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
