//! The D-Bus interfaces on `/org/dmail`. zbus runs them on its own thread;
//! anything that needs the modem goes to the core as a `Command`. Messages
//! are read from the store directly.

use std::sync::{Arc, Mutex};

use async_channel::Sender;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, interface};

use crate::config::{SIM_SLOTS, Settings};
use crate::sms::pdu;
use crate::sms::store::{Blocked, Conversation, Entry, Found, Store};

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
    SetSimSlot(u8, Reply),
    SetDeliveryReports(bool, Reply),
    /// Send the stored outgoing message with this id.
    Send(u64),
    Dial(String, Sender<Result<u8, Error>>),
    Answer(u8, Reply),
    Hangup(u8, Reply),
    SendDtmf(u8, String, Reply),
}

/// Sends a command to the core and waits for its answer.
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataProps {
    pub enabled: bool,
    pub state: String,
    pub interface: String,
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

    async fn set_enabled(&self, enabled: bool) -> Result<(), Error> {
        ask(&self.commands, |reply| Command::SetData(enabled, reply)).await
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

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The core's handle on the interfaces: serves them and publishes changes.
#[derive(Clone)]
pub struct Bus {
    conn: Connection,
}

impl Bus {
    pub async fn serve(
        commands: Sender<Command>,
        store: Arc<Mutex<Store>>,
        settings: Settings,
    ) -> zbus::Result<Self> {
        let data = DataProps {
            enabled: settings.data_enabled,
            state: if settings.data_enabled {
                "connecting"
            } else {
                "off"
            }
            .into(),
            interface: String::new(),
        };
        let conn = zbus::connection::Builder::system()?
            .serve_at(
                PATH,
                Modem {
                    props: ModemProps::starting(settings.sim_slot),
                    commands: commands.clone(),
                },
            )?
            .serve_at(
                PATH,
                Data {
                    props: data,
                    commands: commands.clone(),
                },
            )?
            .serve_at(
                PATH,
                Messages {
                    store,
                    commands: commands.clone(),
                    delivery_reports: settings.delivery_reports,
                },
            )?
            .serve_at(
                PATH,
                Voice {
                    calls: Vec::new(),
                    commands,
                },
            )?
            .name(NAME)?
            .build()
            .await?;
        Ok(Self { conn })
    }

    /// Sets the Modem1 properties, announcing the ones that changed.
    pub async fn publish_modem(&self, p: &ModemProps) -> zbus::Result<()> {
        let iface = self
            .conn
            .object_server()
            .interface::<_, Modem>(PATH)
            .await?;
        let mut m = iface.get_mut().await;
        if m.props == *p {
            return Ok(());
        }
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
        Ok(())
    }

    /// Sets the Data1 properties, announcing the ones that changed.
    pub async fn publish_data(&self, p: &DataProps) -> zbus::Result<()> {
        let iface = self.conn.object_server().interface::<_, Data>(PATH).await?;
        let mut d = iface.get_mut().await;
        if d.props == *p {
            return Ok(());
        }
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
        Ok(())
    }

    /// Sets Messages1.DeliveryReports, announcing a change.
    pub async fn publish_delivery_reports(&self, enabled: bool) -> zbus::Result<()> {
        let iface = self
            .conn
            .object_server()
            .interface::<_, Messages>(PATH)
            .await?;
        let mut m = iface.get_mut().await;
        if m.delivery_reports != enabled {
            m.delivery_reports = enabled;
            m.delivery_reports_changed(iface.signal_emitter()).await?;
        }
        Ok(())
    }

    pub async fn message_received(&self, id: u64, peer: &str) -> zbus::Result<()> {
        let e = SignalEmitter::new(&self.conn, PATH)?;
        Messages::message_received(&e, id, peer).await
    }

    pub async fn message_changed(&self, id: u64, state: &str) -> zbus::Result<()> {
        let e = SignalEmitter::new(&self.conn, PATH)?;
        Messages::message_changed(&e, id, state).await
    }

    /// Sets Voice1.Calls, announcing a change.
    pub async fn publish_calls(&self, calls: Vec<Call>) -> zbus::Result<()> {
        let iface = self.conn.object_server().interface::<_, Voice>(PATH).await?;
        let mut v = iface.get_mut().await;
        if v.calls != calls {
            v.calls = calls;
            v.calls_changed(iface.signal_emitter()).await?;
        }
        Ok(())
    }

    pub async fn call_ended(&self, c: &Call) -> zbus::Result<()> {
        let e = SignalEmitter::new(&self.conn, PATH)?;
        Voice::call_ended(&e, c.0, &c.1, &c.2, c.4).await
    }
}
