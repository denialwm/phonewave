//! The D-Bus interfaces on `/org/dmail`. zbus runs them on its own thread;
//! anything that needs the modem goes to the core as a `Command`. Messages
//! are read from the store directly.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use async_channel::{Receiver, Sender};
use async_executor::LocalExecutor;
use futures_lite::StreamExt;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, interface};

use crate::config::{SIM_SLOTS, Settings};
use crate::sms::pdu;
use crate::sms::store::{Blocked, Conversation, Entry, Found, LoggedCall, Store};

pub const NAME: &str = "org.dmail";
pub const PATH: &str = "/org/dmail";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.dmail.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotSupported(String),
    NotReady(String),
    InvalidArgs(String),
    WrongPin(String),
    Failed(String),
}

pub type Reply = Sender<Result<(), Error>>;

pub enum Command {
    UnlockPin(String, Reply),
    SetData(bool, Reply),
    /// End the data session and start a new one.
    Reconnect(Reply),
    /// Radio off and on.
    RestartRadio(Reply),
    /// Start the modem over, from its setup.
    Restart(Reply),
    SetSimSlot(u8, Reply),
    SetDeliveryReports(bool, Reply),
    /// Send the stored outgoing message with this id.
    Send(u64),
    Dial(String, Sender<Result<u8, Error>>),
    Answer(u8, Reply),
    Hangup(u8, Reply),
    SendDtmf(u8, String, Reply),
    /// This bus name wants location, until it stops or leaves the bus.
    LocationStart(String, Reply),
    LocationStop(String, Reply),
    /// A bus name left the bus.
    NameLost(String),
}

/// Sends a command to the core and waits for its answer. zbus keeps the
/// interface locked until then: see `Bus`.
async fn ask<T>(
    commands: &Sender<Command>,
    make: impl FnOnce(Sender<Result<T, Error>>) -> Command,
) -> Result<T, Error> {
    let (tx, rx) = async_channel::bounded(1);
    let gone = || Error::Failed("dmail is stopping".into());
    commands.send(make(tx)).await.map_err(|_| gone())?;
    rx.recv().await.map_err(|_| gone())?
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModemProps {
    pub state: String,
    pub technology: String,
    pub signal: u8,
    pub operator: String,
    pub roaming: bool,
    pub pin_retries: u32,
    pub sim_slot: u8,
    pub number: String,
    pub attached: bool,
    /// VoLTE: off, registering, registered.
    pub ims: String,
    /// What the network last refused and when; empty and 0 for nothing.
    pub last_reject: (String, i64),
}

impl ModemProps {
    /// A modem coming up, with the SIM in `sim_slot`.
    pub fn starting(sim_slot: u8) -> Self {
        Self {
            state: "starting".into(),
            technology: "none".into(),
            signal: 0,
            operator: String::new(),
            roaming: false,
            pin_retries: 0,
            sim_slot,
            number: String::new(),
            attached: false,
            ims: "off".into(),
            last_reject: (String::new(), 0),
        }
    }
}

pub struct Modem {
    props: ModemProps,
    commands: Sender<Command>,
}

#[interface(name = "org.dmail.Modem1")]
impl Modem {
    /// off, starting, no-sim, locked, searching, registered or failed.
    #[zbus(property)]
    fn state(&self) -> String {
        self.props.state.clone()
    }

    /// none, 2g, 3g, 4g or 5g.
    #[zbus(property)]
    fn technology(&self) -> String {
        self.props.technology.clone()
    }

    /// 0 to 100.
    #[zbus(property)]
    fn signal(&self) -> u8 {
        self.props.signal
    }

    #[zbus(property)]
    fn operator(&self) -> String {
        self.props.operator.clone()
    }

    #[zbus(property)]
    fn roaming(&self) -> bool {
        self.props.roaming
    }

    #[zbus(property)]
    fn pin_retries(&self) -> u32 {
        self.props.pin_retries
    }

    /// The slot whose SIM carries SMS and data, 1-based.
    #[zbus(property)]
    fn sim_slot(&self) -> u8 {
        self.props.sim_slot
    }

    /// The SIM's own number as the SIM holds it; empty when it holds none
    /// or is not open.
    #[zbus(property)]
    fn number(&self) -> String {
        self.props.number.clone()
    }

    /// Registered with packet service attached: mobile data can start.
    #[zbus(property)]
    fn attached(&self) -> bool {
        self.props.attached
    }

    /// VoLTE: off, registering, registered. Off on a phone without it.
    #[zbus(property)]
    fn ims(&self) -> String {
        self.props.ims.clone()
    }

    /// The last registration the network refused, in words, and when (Unix
    /// time); empty and 0 while there was none. Kept when the modem starts
    /// over.
    #[zbus(property)]
    fn last_reject(&self) -> (String, i64) {
        self.props.last_reject.clone()
    }

    /// One attempt. The PIN is never stored.
    async fn unlock_pin(&self, pin: String) -> Result<(), Error> {
        if !(4..=8).contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::InvalidArgs("a PIN is 4 to 8 digits".into()));
        }
        ask(&self.commands, |reply| Command::UnlockPin(pin, reply)).await
    }

    /// Saved; SMS and data move to that slot's SIM at once.
    async fn set_sim_slot(&self, slot: u8) -> Result<(), Error> {
        if !SIM_SLOTS.contains(&slot) {
            return Err(Error::InvalidArgs(format!(
                "slots are {} to {}",
                SIM_SLOTS.start(),
                SIM_SLOTS.end()
            )));
        }
        ask(&self.commands, |reply| Command::SetSimSlot(slot, reply)).await
    }

    /// Turns the radio off and on, which ends calls and data for a few
    /// seconds. The modem registers anew and forgets a domain the network
    /// refused. Refused during a call.
    async fn restart_radio(&self) -> Result<(), Error> {
        ask(&self.commands, Command::RestartRadio).await
    }

    /// Starts the modem over: the phone's modem-setup, then the SIM, the
    /// network and data, as when dmail starts. Also the way out of `failed`.
    /// Refused during a call.
    async fn restart(&self) -> Result<(), Error> {
        ask(&self.commands, Command::Restart).await
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataProps {
    pub enabled: bool,
    pub state: String,
    pub interface: String,
    pub apn: String,
    pub address: String,
    pub dns: Vec<String>,
    pub mtu: u32,
    pub since: i64,
    pub retry_at: i64,
    pub drops: u32,
    pub last_failure: (String, i64),
}

impl DataProps {
    /// What belongs to a running session goes when it does.
    pub fn end_session(&mut self) {
        self.interface.clear();
        self.address.clear();
        self.dns.clear();
        self.mtu = 0;
        self.since = 0;
    }
}

pub struct Data {
    props: DataProps,
    commands: Sender<Command>,
}

#[interface(name = "org.dmail.Data1")]
impl Data {
    #[zbus(property)]
    fn enabled(&self) -> bool {
        self.props.enabled
    }

    /// off, connecting, connected or failed.
    #[zbus(property)]
    fn state(&self) -> String {
        self.props.state.clone()
    }

    #[zbus(property)]
    fn interface(&self) -> String {
        self.props.interface.clone()
    }

    /// The APN in use or being tried; empty before the first try.
    #[zbus(property)]
    fn apn(&self) -> String {
        self.props.apn.clone()
    }

    /// The session's IPv4 address with its prefix length, "10.0.0.2/30";
    /// empty unless connected.
    #[zbus(property)]
    fn address(&self) -> String {
        self.props.address.clone()
    }

    /// The carrier's DNS servers for the session.
    #[zbus(property)]
    fn dns(&self) -> Vec<String> {
        self.props.dns.clone()
    }

    #[zbus(property)]
    fn mtu(&self) -> u32 {
        self.props.mtu
    }

    /// When the session came up (Unix time); 0 unless connected.
    #[zbus(property)]
    fn since(&self) -> i64 {
        self.props.since
    }

    /// When dmail tries again (Unix time); 0 while no try is waiting.
    #[zbus(property)]
    fn retry_at(&self) -> i64 {
        self.props.retry_at
    }

    /// Sessions the network ended since dmail started.
    #[zbus(property)]
    fn drops(&self) -> u32 {
        self.props.drops
    }

    /// Why the last session ended or failed to start, and when (Unix time);
    /// empty and 0 while none did.
    #[zbus(property)]
    fn last_failure(&self) -> (String, i64) {
        self.props.last_failure.clone()
    }

    async fn set_enabled(&self, enabled: bool) -> Result<(), Error> {
        ask(&self.commands, |reply| Command::SetData(enabled, reply)).await
    }

    /// Ends the session and starts a new one at once, also after the
    /// network refused packet service. Mobile data must be on.
    async fn reconnect(&self) -> Result<(), Error> {
        ask(&self.commands, Command::Reconnect).await
    }
}

pub struct Messages {
    store: Arc<Mutex<Store>>,
    commands: Sender<Command>,
    delivery_reports: bool,
}

fn store_error(e: rusqlite::Error) -> Error {
    Error::Failed(format!("message store: {e}"))
}

/// `before` as the store takes it: 0 means the newest.
fn before_or_newest(before: i64) -> i64 {
    if before <= 0 { i64::MAX } else { before }
}

#[interface(name = "org.dmail.Messages1")]
impl Messages {
    /// Whether Send asks the network for a status report on every part.
    #[zbus(property)]
    fn delivery_reports(&self) -> bool {
        self.delivery_reports
    }

    /// Saved; applies to messages sent from then on.
    async fn set_delivery_reports(&self, enabled: bool) -> Result<(), Error> {
        ask(&self.commands, |reply| {
            Command::SetDeliveryReports(enabled, reply)
        })
        .await
    }

    /// Peer; the last message's text, time and whether it was outgoing; the
    /// unread count. Newest first, archived conversations left out.
    fn list_conversations(&self) -> Result<Vec<Conversation>, Error> {
        self.store
            .lock()
            .unwrap()
            .conversations(false)
            .map_err(store_error)
    }

    /// The archived conversations, as ListConversations lists the others.
    fn list_archived_conversations(&self) -> Result<Vec<Conversation>, Error> {
        self.store
            .lock()
            .unwrap()
            .conversations(true)
            .map_err(store_error)
    }

    /// Id, outgoing, text, time, state; newest first, older than `before`
    /// (0 for the newest).
    fn list_messages(&self, peer: String, before: i64, limit: u32) -> Result<Vec<Entry>, Error> {
        self.store
            .lock()
            .unwrap()
            .messages(&peer, before_or_newest(before), limit)
            .map_err(store_error)
    }

    /// Stores the message as `sending` and returns its id; MessageChanged
    /// follows with `sent` or `failed`, and with delivery reports on, later
    /// `delivered`, `undelivered` or `expired`.
    async fn send(&self, peer: String, text: String) -> Result<u64, Error> {
        if text.is_empty() {
            return Err(Error::InvalidArgs("empty message".into()));
        }
        pdu::encode_submit(&peer, &text, 0, false)
            .map_err(|e| Error::InvalidArgs(format!("{e:?}")))?;
        let id = self
            .store
            .lock()
            .unwrap()
            .add_outgoing(&peer, &text, now())
            .map_err(store_error)?;
        self.commands
            .send(Command::Send(id))
            .await
            .map_err(|_| Error::Failed("dmail is stopping".into()))?;
        Ok(id)
    }

    /// ConversationRead follows, so the notification can go.
    async fn mark_read(
        &self,
        peer: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        let peer = self
            .store
            .lock()
            .unwrap()
            .mark_read(&peer)
            .map_err(store_error)?;
        Self::conversation_read(&emitter, &peer).await?;
        Ok(())
    }

    fn delete(&self, id: u64) -> Result<(), Error> {
        match self.store.lock().unwrap().delete(id).map_err(store_error)? {
            true => Ok(()),
            false => Err(Error::InvalidArgs(format!("no message {id}"))),
        }
    }

    /// Deletes every message with `peer`; ConversationDeleted follows.
    async fn delete_conversation(
        &self,
        peer: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        let deleted = self
            .store
            .lock()
            .unwrap()
            .delete_conversation(&peer)
            .map_err(store_error)?;
        let peer = deleted.ok_or_else(|| Error::InvalidArgs(format!("no conversation {peer}")))?;
        Self::conversation_deleted(&emitter, &peer).await?;
        Ok(())
    }

    /// ConversationArchived follows. A new message in an archived
    /// conversation brings it back.
    async fn set_archived(
        &self,
        peer: String,
        archived: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        let changed = self
            .store
            .lock()
            .unwrap()
            .set_archived(&peer, archived)
            .map_err(store_error)?;
        let peer = changed.ok_or_else(|| Error::InvalidArgs(format!("no conversation {peer}")))?;
        Self::conversation_archived(&emitter, &peer, archived).await?;
        Ok(())
    }

    /// Messages with, for each word of `text`, a word starting with it:
    /// peer, then id, outgoing, text, time, state. Newest first.
    fn search(&self, text: String, limit: u32) -> Result<Vec<Found>, Error> {
        self.store
            .lock()
            .unwrap()
            .search(&text, limit)
            .map_err(store_error)
    }

    /// Messages from this number, or sender name, are still received, but go
    /// to the blocked folder: no conversation, no MessageReceived. Numbers
    /// match on their last 9 digits, as conversations do. BlockedChanged
    /// follows.
    async fn block(
        &self,
        number: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        if number.is_empty() {
            return Err(Error::InvalidArgs("empty number".into()));
        }
        self.store
            .lock()
            .unwrap()
            .block(&number)
            .map_err(store_error)?;
        Self::blocked_changed(&emitter).await?;
        Ok(())
    }

    /// Removes every blocked entry that matches `number`; what it sent goes
    /// back to its conversation, read. BlockedChanged follows.
    async fn unblock(
        &self,
        number: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        self.store
            .lock()
            .unwrap()
            .unblock(&number)
            .map_err(store_error)?;
        Self::blocked_changed(&emitter).await?;
        Ok(())
    }

    fn list_blocked(&self) -> Result<Vec<String>, Error> {
        self.store.lock().unwrap().blocked().map_err(store_error)
    }

    /// The blocked folder: id, sender, text, time; newest first, older than
    /// `before` (0 for the newest). Delete takes these ids too.
    fn list_blocked_messages(&self, before: i64, limit: u32) -> Result<Vec<Blocked>, Error> {
        self.store
            .lock()
            .unwrap()
            .blocked_messages(before_or_newest(before), limit)
            .map_err(store_error)
    }

    #[zbus(signal)]
    pub async fn message_received(
        emitter: &SignalEmitter<'_>,
        id: u64,
        peer: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn message_changed(
        emitter: &SignalEmitter<'_>,
        id: u64,
        state: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn conversation_read(emitter: &SignalEmitter<'_>, peer: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn conversation_deleted(emitter: &SignalEmitter<'_>, peer: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn conversation_archived(
        emitter: &SignalEmitter<'_>,
        peer: &str,
        archived: bool,
    ) -> zbus::Result<()>;

    /// The block list changed.
    #[zbus(signal)]
    async fn blocked_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Id, number (empty when withheld), direction (`in` or `out`), state, and
/// when it connected (0 until then).
pub type Call = (u8, String, String, String, i64);

pub struct Voice {
    calls: Vec<Call>,
    commands: Sender<Command>,
    store: Arc<Mutex<Store>>,
}

#[interface(name = "org.dmail.Voice1")]
impl Voice {
    /// States: dialing, alerting, incoming, waiting, active, held, ending.
    /// Ended calls leave the list.
    #[zbus(property)]
    fn calls(&self) -> Vec<Call> {
        self.calls.clone()
    }

    /// Returns the new call's id.
    async fn dial(&self, number: String) -> Result<u8, Error> {
        let dialable = |b: u8| b.is_ascii_digit() || b"+*#".contains(&b);
        if number.is_empty() || !number.bytes().all(dialable) {
            return Err(Error::InvalidArgs("a number is digits, +, * and #".into()));
        }
        ask(&self.commands, |reply| Command::Dial(number, reply)).await
    }

    async fn answer(&self, id: u8) -> Result<(), Error> {
        ask(&self.commands, |reply| Command::Answer(id, reply)).await
    }

    /// Ends a call, or turns down an incoming one.
    async fn hangup(&self, id: u8) -> Result<(), Error> {
        ask(&self.commands, |reply| Command::Hangup(id, reply)).await
    }

    /// Plays the digits to the other side; returns once they have sounded.
    async fn send_dtmf(&self, id: u8, digits: String) -> Result<(), Error> {
        let tone = |b: u8| b.is_ascii_digit() || b"*#".contains(&b);
        if digits.is_empty() || !digits.bytes().all(tone) {
            return Err(Error::InvalidArgs("DTMF digits are 0 to 9, * and #".into()));
        }
        ask(&self.commands, |reply| Command::SendDtmf(id, digits, reply)).await
    }

    /// Up to `limit` calls of the log with ids below `before` (0 for the
    /// newest), newest first: id, number (empty when withheld), direction
    /// (`in` or `out`), outcome (answered, missed, declined, unanswered), and
    /// the Unix times it started, connected (0 if it never did) and ended.
    fn list_call_log(&self, before: u64, limit: u32) -> Result<Vec<LoggedCall>, Error> {
        self.store
            .lock()
            .unwrap()
            .call_log(before, limit)
            .map_err(store_error)
    }

    /// Takes a call off the log; CallLogDeleted follows.
    async fn delete_logged_call(
        &self,
        id: u64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        let deleted = self
            .store
            .lock()
            .unwrap()
            .delete_logged_call(id)
            .map_err(store_error)?;
        if !deleted {
            return Err(Error::InvalidArgs(format!("no logged call {id}")));
        }
        Self::call_log_deleted(&emitter, id).await?;
        Ok(())
    }

    /// A call that ended went into the log under `id`.
    #[zbus(signal)]
    pub async fn call_logged(emitter: &SignalEmitter<'_>, id: u64) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn call_log_deleted(emitter: &SignalEmitter<'_>, id: u64) -> zbus::Result<()>;

    /// A call left the list. `connected` is 0 for a call never answered: an
    /// incoming one was missed or turned down.
    #[zbus(signal)]
    pub async fn call_ended(
        emitter: &SignalEmitter<'_>,
        id: u8,
        number: &str,
        direction: &str,
        connected: i64,
    ) -> zbus::Result<()>;
}

/// A fix as Location1 publishes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres, radius of 68% confidence.
    pub accuracy: f64,
    /// Metres, one standard deviation (68%), as all three below.
    pub vertical_accuracy: Option<f64>,
    /// Metres a second.
    pub speed_accuracy: Option<f64>,
    /// Degrees.
    pub heading_accuracy: Option<f64>,
    /// Metres above the WGS 84 ellipsoid.
    pub altitude: Option<f64>,
    /// Metres a second.
    pub speed: Option<f64>,
    /// Degrees from true north.
    pub heading: Option<f64>,
    /// Unix milliseconds.
    pub time: i64,
}

/// One signal of a satellite, as `Satellites` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Satellite {
    /// gps, glonass, galileo, beidou, qzss, sbas, navic.
    pub system: &'static str,
    /// Its number within the constellation.
    pub number: u16,
    /// The band, such as L1 C/A or E5a; empty when unknown.
    pub band: &'static str,
    /// Carrier frequency, Hz.
    pub frequency: Option<f64>,
    /// dB-Hz, while heard.
    pub cn0: Option<f64>,
    /// Degrees.
    pub elevation: Option<f64>,
    pub azimuth: Option<f64>,
    pub tracked: bool,
    pub used: bool,
    pub ephemeris: bool,
    pub almanac: bool,
}

pub type SatelliteEntry = (
    String,
    u16,
    String,
    f64,
    f64,
    f64,
    f64,
    bool,
    bool,
    bool,
    bool,
);

#[derive(Clone, Debug, PartialEq)]
pub struct LocationProps {
    /// off, searching, fixed, unavailable.
    pub state: String,
    pub fix: Option<Fix>,
    pub satellites_used: u32,
    pub satellites_tracked: u32,
    pub satellites: Vec<Satellite>,
}

impl LocationProps {
    pub fn with_state(state: &str) -> Self {
        Self {
            state: state.into(),
            fix: None,
            satellites_used: 0,
            satellites_tracked: 0,
            satellites: Vec::new(),
        }
    }
}

pub type Position = (f64, f64, f64, f64, f64, f64, i64);

pub struct Location {
    props: LocationProps,
    commands: Sender<Command>,
}

fn sender(header: &Header<'_>) -> Result<String, Error> {
    header
        .sender()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::InvalidArgs("no sender".into()))
}

#[interface(name = "org.dmail.Location1")]
impl Location {
    /// off: nobody asks; searching: no fix yet; fixed: Position is current;
    /// unavailable: the modem has no GNSS engine, or failed.
    #[zbus(property)]
    fn state(&self) -> String {
        self.props.state.clone()
    }

    /// Latitude and longitude in degrees, accuracy in metres, altitude above
    /// the WGS 84 ellipsoid in metres, speed in metres a second, heading in
    /// degrees from true north, and the fix's Unix time in milliseconds.
    /// NaN for what the fix lacks; all NaN and 0 before the first fix.
    #[zbus(property)]
    fn position(&self) -> Position {
        let Some(f) = &self.props.fix else {
            return (
                f64::NAN,
                f64::NAN,
                f64::NAN,
                f64::NAN,
                f64::NAN,
                f64::NAN,
                0,
            );
        };
        let or_nan = |v: Option<f64>| v.unwrap_or(f64::NAN);
        (
            f.latitude,
            f.longitude,
            f.accuracy,
            or_nan(f.altitude),
            or_nan(f.speed),
            or_nan(f.heading),
            f.time,
        )
    }

    /// Vertical, speed and heading accuracy of the fix in Position: metres,
    /// metres a second and degrees, one standard deviation. NaN for what
    /// the fix lacks.
    #[zbus(property)]
    fn accuracies(&self) -> (f64, f64, f64) {
        let fix = self.props.fix.as_ref();
        let or_nan = |v: Option<f64>| v.unwrap_or(f64::NAN);
        (
            or_nan(fix.and_then(|f| f.vertical_accuracy)),
            or_nan(fix.and_then(|f| f.speed_accuracy)),
            or_nan(fix.and_then(|f| f.heading_accuracy)),
        )
    }

    /// Every signal the engine lists, one entry for each satellite and
    /// band: constellation, number within it, band, carrier frequency (Hz),
    /// C/N0 (dB-Hz), elevation, azimuth (degrees), tracked, used in the last
    /// fix, has ephemeris, has almanac. NaN for what is unknown; C/N0 NaN
    /// while not heard.
    #[zbus(property)]
    fn satellites(&self) -> Vec<SatelliteEntry> {
        let or_nan = |v: Option<f64>| v.unwrap_or(f64::NAN);
        self.props
            .satellites
            .iter()
            .map(|s| {
                (
                    s.system.to_owned(),
                    s.number,
                    s.band.to_owned(),
                    or_nan(s.frequency),
                    or_nan(s.cn0),
                    or_nan(s.elevation),
                    or_nan(s.azimuth),
                    s.tracked,
                    s.used,
                    s.ephemeris,
                    s.almanac,
                )
            })
            .collect()
    }

    /// Satellites the last fix used.
    #[zbus(property)]
    fn satellites_used(&self) -> u32 {
        self.props.satellites_used
    }

    /// Satellites the engine hears now; it searches for more while 0.
    #[zbus(property)]
    fn satellites_tracked(&self) -> u32 {
        self.props.satellites_tracked
    }

    /// The caller wants location until Stop or until it leaves the bus. The
    /// engine runs while anyone wants it.
    async fn start(&self, #[zbus(header)] header: Header<'_>) -> Result<(), Error> {
        let name = sender(&header)?;
        ask(&self.commands, |reply| Command::LocationStart(name, reply)).await
    }

    async fn stop(&self, #[zbus(header)] header: Header<'_>) -> Result<(), Error> {
        let name = sender(&header)?;
        ask(&self.commands, |reply| Command::LocationStop(name, reply)).await
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// What the core hands the publisher, in the order it happened.
enum Out {
    Modem(ModemProps),
    Data(DataProps),
    Calls(Vec<Call>),
    DeliveryReports(bool),
    MessageReceived(u64, String),
    MessageChanged(u64, String),
    CallEnded(Call),
    CallLogged(u64),
    Location(LocationProps),
}

/// The core's handle on the interfaces: serves them and publishes changes.
///
/// zbus keeps an interface locked while one of its methods waits for the
/// core's answer, so the core never takes an interface's lock itself: it
/// hands every change to one publisher task, which applies them in order
/// once the lock is free. It keeps what it last handed over, to read back.
#[derive(Clone)]
pub struct Bus {
    conn: Connection,
    out: Sender<Out>,
    modem: Rc<RefCell<ModemProps>>,
    data: Rc<RefCell<DataProps>>,
    location: Rc<RefCell<LocationProps>>,
}

impl Bus {
    pub async fn serve(
        ex: &LocalExecutor<'static>,
        commands: Sender<Command>,
        store: Arc<Mutex<Store>>,
        settings: Settings,
    ) -> zbus::Result<Self> {
        let modem = ModemProps::starting(settings.sim_slot);
        let data = DataProps {
            enabled: settings.data_enabled,
            state: if settings.data_enabled {
                "connecting"
            } else {
                "off"
            }
            .into(),
            ..DataProps::default()
        };
        let location = LocationProps::with_state("off");
        let conn = zbus::connection::Builder::system()?
            .serve_at(
                PATH,
                Modem {
                    props: modem.clone(),
                    commands: commands.clone(),
                },
            )?
            .serve_at(
                PATH,
                Data {
                    props: data.clone(),
                    commands: commands.clone(),
                },
            )?
            .serve_at(
                PATH,
                Messages {
                    store: store.clone(),
                    commands: commands.clone(),
                    delivery_reports: settings.delivery_reports,
                },
            )?
            .serve_at(
                PATH,
                Voice {
                    calls: Vec::new(),
                    commands: commands.clone(),
                    store,
                },
            )?
            .serve_at(
                PATH,
                Location {
                    props: location.clone(),
                    commands: commands.clone(),
                },
            )?
            .name(NAME)?
            .build()
            .await?;
        let (out, changes) = async_channel::unbounded();
        ex.spawn(publish(conn.clone(), changes)).detach();
        ex.spawn(follow_names(conn.clone(), commands)).detach();
        Ok(Self {
            conn,
            out,
            modem: Rc::new(RefCell::new(modem)),
            data: Rc::new(RefCell::new(data)),
            location: Rc::new(RefCell::new(location)),
        })
    }

    fn send(&self, out: Out) {
        // Closed only once dmail stops.
        let _ = self.out.try_send(out);
    }

    /// Sets the Modem1 properties, announcing the ones that changed.
    pub fn publish_modem(&self, p: &ModemProps) {
        if *self.modem.borrow() != *p {
            self.modem.replace(p.clone());
            self.send(Out::Modem(p.clone()));
        }
    }

    /// Modem1.LastReject as last published.
    pub fn last_reject(&self) -> (String, i64) {
        self.modem.borrow().last_reject.clone()
    }

    /// Changes the Data1 properties, announcing the ones that changed.
    pub fn update_data(&self, change: impl FnOnce(&mut DataProps)) {
        let mut data = self.data.borrow_mut();
        let old = data.clone();
        change(&mut data);
        if *data != old {
            self.send(Out::Data(data.clone()));
        }
    }

    /// The system bus connection, for talking to other services.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Sets Messages1.DeliveryReports, announcing a change.
    pub fn publish_delivery_reports(&self, enabled: bool) {
        self.send(Out::DeliveryReports(enabled));
    }

    pub fn message_received(&self, id: u64, peer: &str) {
        self.send(Out::MessageReceived(id, peer.into()));
    }

    pub fn message_changed(&self, id: u64, state: &str) {
        self.send(Out::MessageChanged(id, state.into()));
    }

    /// Sets Voice1.Calls, announcing a change.
    pub fn publish_calls(&self, calls: Vec<Call>) {
        self.send(Out::Calls(calls));
    }

    pub fn call_ended(&self, c: &Call) {
        self.send(Out::CallEnded(c.clone()));
    }

    pub fn call_logged(&self, id: u64) {
        self.send(Out::CallLogged(id));
    }

    /// Changes the Location1 properties, announcing the ones that changed.
    pub fn update_location(&self, change: impl FnOnce(&mut LocationProps)) {
        let mut location = self.location.borrow_mut();
        let old = location.clone();
        change(&mut location);
        if *location != old {
            self.send(Out::Location(location.clone()));
        }
    }
}

/// Tells the core about every client that leaves the bus, so it can let go of
/// what the client asked for.
async fn follow_names(conn: Connection, commands: Sender<Command>) {
    let stream = async {
        let proxy = zbus::fdo::DBusProxy::new(&conn).await?;
        proxy.receive_name_owner_changed().await
    };
    let mut changes = match stream.await {
        Ok(changes) => changes,
        Err(e) => {
            eprintln!("dmail: following bus names: {e}");
            return;
        }
    };
    while let Some(change) = changes.next().await {
        let Ok(args) = change.args() else {
            continue;
        };
        if args.new_owner().is_none() && args.name().starts_with(':') {
            let name = args.name().to_string();
            if commands.send(Command::NameLost(name)).await.is_err() {
                return;
            }
        }
    }
}

/// The publisher: applies the core's changes one by one.
async fn publish(conn: Connection, changes: Receiver<Out>) {
    while let Ok(out) = changes.recv().await {
        if let Err(e) = apply(&conn, out).await {
            eprintln!("dmail: publishing: {e}");
        }
    }
}

async fn apply(conn: &Connection, out: Out) -> zbus::Result<()> {
    let server = conn.object_server();
    match out {
        Out::Modem(p) => {
            let iface = server.interface::<_, Modem>(PATH).await?;
            let mut m = iface.get_mut().await;
            let old = std::mem::replace(&mut m.props, p.clone());
            let e = iface.signal_emitter();
            if old.state != p.state {
                m.state_changed(e).await?;
            }
            if old.technology != p.technology {
                m.technology_changed(e).await?;
            }
            if old.signal != p.signal {
                m.signal_changed(e).await?;
            }
            if old.operator != p.operator {
                m.operator_changed(e).await?;
            }
            if old.roaming != p.roaming {
                m.roaming_changed(e).await?;
            }
            if old.pin_retries != p.pin_retries {
                m.pin_retries_changed(e).await?;
            }
            if old.sim_slot != p.sim_slot {
                m.sim_slot_changed(e).await?;
            }
            if old.number != p.number {
                m.number_changed(e).await?;
            }
            if old.attached != p.attached {
                m.attached_changed(e).await?;
            }
            if old.last_reject != p.last_reject {
                m.last_reject_changed(e).await?;
            }
        }
        Out::Data(p) => {
            let iface = server.interface::<_, Data>(PATH).await?;
            let mut d = iface.get_mut().await;
            let old = std::mem::replace(&mut d.props, p.clone());
            let e = iface.signal_emitter();
            if old.enabled != p.enabled {
                d.enabled_changed(e).await?;
            }
            if old.state != p.state {
                d.state_changed(e).await?;
            }
            if old.interface != p.interface {
                d.interface_changed(e).await?;
            }
            if old.apn != p.apn {
                d.apn_changed(e).await?;
            }
            if old.address != p.address {
                d.address_changed(e).await?;
            }
            if old.dns != p.dns {
                d.dns_changed(e).await?;
            }
            if old.mtu != p.mtu {
                d.mtu_changed(e).await?;
            }
            if old.since != p.since {
                d.since_changed(e).await?;
            }
            if old.retry_at != p.retry_at {
                d.retry_at_changed(e).await?;
            }
            if old.drops != p.drops {
                d.drops_changed(e).await?;
            }
            if old.last_failure != p.last_failure {
                d.last_failure_changed(e).await?;
            }
        }
        Out::Calls(calls) => {
            let iface = server.interface::<_, Voice>(PATH).await?;
            let mut v = iface.get_mut().await;
            if v.calls != calls {
                v.calls = calls;
                v.calls_changed(iface.signal_emitter()).await?;
            }
        }
        Out::DeliveryReports(enabled) => {
            let iface = server.interface::<_, Messages>(PATH).await?;
            let mut m = iface.get_mut().await;
            if m.delivery_reports != enabled {
                m.delivery_reports = enabled;
                m.delivery_reports_changed(iface.signal_emitter()).await?;
            }
        }
        Out::MessageReceived(id, peer) => {
            let e = SignalEmitter::new(conn, PATH)?;
            Messages::message_received(&e, id, &peer).await?;
        }
        Out::MessageChanged(id, state) => {
            let e = SignalEmitter::new(conn, PATH)?;
            Messages::message_changed(&e, id, &state).await?;
        }
        Out::CallEnded(c) => {
            let e = SignalEmitter::new(conn, PATH)?;
            Voice::call_ended(&e, c.0, &c.1, &c.2, c.4).await?;
        }
        Out::CallLogged(id) => {
            let e = SignalEmitter::new(conn, PATH)?;
            Voice::call_logged(&e, id).await?;
        }
        Out::Location(p) => {
            let iface = server.interface::<_, Location>(PATH).await?;
            let mut l = iface.get_mut().await;
            let old = std::mem::replace(&mut l.props, p.clone());
            let e = iface.signal_emitter();
            if old.state != p.state {
                l.state_changed(e).await?;
            }
            // Accuracies first: a client that keeps them has the fix's own
            // when Position changes.
            if old.fix != p.fix {
                l.accuracies_changed(e).await?;
                l.position_changed(e).await?;
            }
            if old.satellites != p.satellites {
                l.satellites_changed(e).await?;
            }
            if old.satellites_used != p.satellites_used {
                l.satellites_used_changed(e).await?;
            }
            if old.satellites_tracked != p.satellites_tracked {
                l.satellites_tracked_changed(e).await?;
            }
        }
    }
    Ok(())
}
