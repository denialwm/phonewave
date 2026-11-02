//! VoLTE. The modem runs IMS itself; dmail gives it a link-local session, the
//! connection manager it looks for, the phone's properties and the bearer it
//! asks for, then follows its registration. See DESIGN.md, VoLTE.

use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddrV6, UdpSocket};
use std::time::{Duration, Instant};

use async_io::{Async, Timer};
use futures_lite::future;

use crate::config::ImsConfig;
use crate::data::{self, Endpoint, Status};
use crate::netlink::{self, Netlink};
use crate::network;
use crate::qmi::{Client, Error, Kind, Message, service};
use crate::qrtr::{self, Addr, Socket};

const TIMEOUT: Duration = Duration::from_secs(10);
/// Starting a session waits for the network.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// After a failed run.
const RESTART: Duration = Duration::from_secs(60);
/// How long an attempt to register gets. From a fresh start the modem asks
/// for the bearer at once; after a refused bearer or a dmail restart it waited
/// 2, 3, then 5 minutes, longer each time (nx809j, 2026-11-02).
const ATTEMPT: Duration = Duration::from_secs(600);
/// Between attempts, doubling up to the last.
const FIRST_PAUSE: Duration = Duration::from_secs(600);
const LAST_PAUSE: Duration = Duration::from_secs(3600);
/// Reject causes that stop packet service: 3GPP TS 24.301, 7, 8 and 14.
const PS_REFUSED: [u8; 3] = [7, 8, 14];

const LINK: &str = "rmnet_ims0";
const LINK_LOCAL_MUX: u8 = 4;
const LINK_LOCAL_APN: &str = "linklocal";
const BEARER_MUX: u8 = 3;
const BEARER_APN: &str = "ims";
/// The modem profile the bearer runs on.
const BEARER_PROFILE: u8 = 2;
const NO_PROFILE: u8 = 0xff;

const SUBSCRIPTION_NONE: u32 = 0xff;
const SUBSCRIPTION_PRIMARY: u32 = 1;
const IPV4: u8 = 4;
const IPV6: u8 = 6;
const CALL_EMBEDDED: u8 = 1;
/// The technology preference Android starts the link-local session with.
const TECHNOLOGY_LINK_LOCAL: u16 = 0x8888;

const WDS_START_NETWORK: u16 = 0x20;
const WDS_GET_CURRENT_SETTINGS: u16 = 0x2d;
const WDS_SET_IP_FAMILY: u16 = 0x4d;
const WDS_BIND_SUBSCRIPTION: u16 = 0xaf;
const QOS: u32 = 0x04;
const QOS_BIND_MUX: u16 = 0x2b;
const QOS_BIND_SUBSCRIPTION: u16 = 0x2d;
const IMSA_REGISTER_INDICATIONS: u16 = 0x22;
const IMSA_REGISTRATION_CHANGED: u16 = 0x23;
const IMSA_BIND: u16 = 0x33;

/// The connection manager the modem looks for on this side.
const DCM: u32 = 0x302;
const DCM_VERSION: u8 = 1;
const DCM_ACTIVATE: u16 = 0x20;
const DCM_DEACTIVATE: u16 = 0x21;
const DCM_LINK_ADDRESS: u16 = 0x23;
/// The bearer's id in the connection manager's messages.
const DCM_BEARER: u8 = 0x15;
/// Result TLVs: success; failure with a general error; and the "down" a
/// running bearer's indication carries.
const DCM_OK: [u8; 4] = [0, 0, 0, 0];
const DCM_FAILED: [u8; 4] = [1, 0, 0x0d, 0];
const DCM_DOWN: [u8; 4] = [0, 0, 0x0d, 0];

/// The handshake over UDP: a header of id, kind and length, then the body.
const HELLO: u32 = 1;
const HELLO_REPLY: u32 = 2;
const PROPERTIES: u32 = 3;
const HELLO_SIDE: u32 = 0xad30;
const PROPERTY_SIZE: usize = 512;
const PROBE_PORT: u16 = 9099;
const PROBE_PORT_2: u16 = 9098;

fn log(what: impl std::fmt::Display) {
    eprintln!("dmail: VoLTE: {what}");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    Off,
    Registering,
    Registered,
}

impl Registration {
    pub fn as_str(self) -> &'static str {
        match self {
            Registration::Off => "off",
            Registration::Registering => "registering",
            Registration::Registered => "registered",
        }
    }
}

/// Runs VoLTE for one life of the modem, from the start again a minute after
/// a failed run. `report` hears every registration change. `parent` is the
/// data port's netdev, already open and framed.
pub async fn run(
    config: ImsConfig,
    parent: String,
    endpoint: Endpoint,
    report: impl Fn(Registration),
) {
    loop {
        report(Registration::Registering);
        let result = match Ims::open(&config, &parent, endpoint).await {
            Ok(mut ims) => ims.serve(&report).await,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            log(format!("{e}; again in a minute"));
        }
        report(Registration::Off);
        Timer::after(RESTART).await;
    }
}

/// The modem's end of the link-local session, from Link Address.
#[derive(Debug, PartialEq, Eq)]
struct ModemEnd {
    address: Ipv6Addr,
    port: u16,
    id: u32,
}

/// Link Address: TLV 0x01 holds the port, then at 6 the address's length and
/// the address as text; TLV 0x12 the handshake id.
fn parse_link_address(msg: &Message) -> Option<ModemEnd> {
    let link = msg.tlv(0x01)?;
    let port = u16::from_le_bytes([*link.first()?, *link.get(1)?]);
    let len = usize::from(*link.get(6)?);
    let text = std::str::from_utf8(link.get(7..7 + len)?).ok()?;
    let id = match *msg.tlv(0x12)? {
        [a, b] => u16::from_le_bytes([a, b]).into(),
        [a, b, c, d] => u32::from_le_bytes([a, b, c, d]),
        _ => return None,
    };
    Some(ModemEnd {
        address: text.trim_end_matches('\0').parse().ok()?,
        port,
        id,
    })
}

/// Activate's APN: a length, then the name.
fn activation_apn(msg: &Message) -> Option<&str> {
    let v = msg.tlv(0x01)?;
    std::str::from_utf8(v.get(1..1 + usize::from(*v.first()?))?).ok()
}

fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn hello(id: u32) -> Vec<u8> {
    words(&[id, HELLO, 20, 1, HELLO_SIDE])
}

/// The properties in the order the modem numbers them, from 1, each in a
/// field of its own size.
fn properties(id: u32, c: &ImsConfig) -> Vec<u8> {
    let values = [&c.manufacturer, &c.model, &c.build, &c.release, &c.sdk];
    let len = 16 + values.len() * (8 + PROPERTY_SIZE);
    let mut out = words(&[id, PROPERTIES, len as u32, values.len() as u32]);
    for (key, value) in (1u32..).zip(values) {
        let value = &value.as_bytes()[..value.len().min(PROPERTY_SIZE)];
        out.extend(words(&[key, value.len() as u32]));
        out.extend_from_slice(value);
        out.resize(out.len() + PROPERTY_SIZE - value.len(), 0);
    }
    out
}

fn probe(c: &ImsConfig) -> Vec<u8> {
    let name = format!("{}_{}_{}\0", c.manufacturer, c.model, c.build);
    let mut out = vec![0x62, 0xdb, 0x14];
    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out
}

fn answer(request: &Message, result: [u8; 4]) -> Message {
    let mut reply = Message::request(request.id).with(0x02, result);
    reply.kind = Kind::Response;
    reply.txn = request.txn;
    reply
}

/// The bearer's state for the modem: up with `address`, or down. The address
/// goes as text after its family, 0 for IPv4 and 1 for IPv6, as Android
/// numbers them.
fn bearer_indication(txn: u16, token: &[u8], address: Option<IpAddr>) -> Message {
    let mut ind = Message::request(DCM_ACTIVATE)
        .with(0x02, if address.is_some() { DCM_OK } else { DCM_DOWN })
        .with(0x01, [DCM_BEARER])
        .with(0x10, token);
    if let Some(address) = address {
        let text = address.to_string();
        let family: u32 = if address.is_ipv6() { 1 } else { 0 };
        let mut value = family.to_le_bytes().to_vec();
        value.push(text.len() as u8);
        value.extend_from_slice(text.as_bytes());
        ind = ind.with(0x11, value);
    }
    ind = ind.with(0x12, 1u32.to_le_bytes());
    ind.kind = Kind::Indication;
    ind.txn = txn;
    ind
}

/// The running session's address: IPv6 in TLV 0x25 with its prefix length.
async fn bearer_address(wds: &Client, family: u8) -> Result<IpAddr, Error> {
    let msg = Message::request(WDS_GET_CURRENT_SETTINGS).with(0x10, 0xffffu32.to_le_bytes());
    let reply = wds.call(msg, TIMEOUT).await?;
    if family == IPV4 {
        return data::parse_settings(&reply).map(|s| s.address.into());
    }
    match reply.tlv(0x25) {
        Some(v) if v.len() >= 16 => {
            let octets: [u8; 16] = v[..16].try_into().unwrap();
            Ok(Ipv6Addr::from(octets).into())
        }
        _ => Err(Error::Malformed("no IPv6 address")),
    }
}

/// A random link-local address: the link only ever talks to the modem.
fn link_local_address() -> io::Result<Ipv6Addr> {
    let mut bytes = [0u8; 16];
    bytes[0..2].copy_from_slice(&[0xfe, 0x80]);
    // SAFETY: the buffer is writable for the length given.
    let n = unsafe { libc::getrandom(bytes[8..].as_mut_ptr().cast(), 8, 0) };
    if n != 8 {
        return Err(io::Error::last_os_error());
    }
    Ok(Ipv6Addr::from(bytes))
}

async fn client(id: u32, name: &str) -> Result<Client, String> {
    let found = qrtr::find(id).await.map_err(|e| format!("{name}: {e}"))?;
    let s = found.ok_or_else(|| format!("no {name} service"))?;
    Client::new(s.addr).map_err(|e| format!("{name}: {e}"))
}

/// A WDS client for one mux, subscription and IP family.
async fn wds_on(
    endpoint: Endpoint,
    mux: u8,
    subscription: u32,
    family: u8,
) -> Result<Client, String> {
    let wds = client(service::WDS, "WDS").await?;
    let steps = async {
        data::bind_mux(&wds, endpoint, mux).await?;
        let bind = Message::request(WDS_BIND_SUBSCRIPTION).with(0x01, subscription.to_le_bytes());
        wds.call(bind, TIMEOUT).await?;
        let msg = Message::request(WDS_SET_IP_FAMILY).with(0x01, [family]);
        wds.call(msg, TIMEOUT).await?;
        Ok::<_, Error>(())
    };
    steps.await.map_err(|e| format!("WDS on mux {mux}: {e}"))?;
    Ok(wds)
}

/// Starts a session; its handle.
async fn start(wds: &Client, msg: Message) -> Result<u32, Error> {
    match wds.call(msg, START_TIMEOUT).await?.tlv(0x01) {
        Some(&[a, b, c, d]) => Ok(u32::from_le_bytes([a, b, c, d])),
        _ => Err(Error::Malformed("no session handle")),
    }
}

/// rmnet_ims0, removed however the run ends.
struct Link(Netlink);

impl Link {
    fn create(parent: &str) -> io::Result<Self> {
        let netlink = Netlink::new()?;
        if netlink::index(LINK).is_ok() {
            netlink.delete_link(LINK)?;
        }
        netlink.add_rmnet(parent, LINK, LINK_LOCAL_MUX.into(), netlink::RMNET_FLAGS)?;
        Ok(Self(netlink))
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        if netlink::index(LINK).is_ok()
            && let Err(e) = self.0.delete_link(LINK)
        {
            log(format!("removing {LINK}: {e}"));
        }
    }
}

/// The bearer the modem asked for, and the token its request carried.
struct Bearer {
    wds: Client,
    handle: u32,
    token: Vec<u8>,
}

/// When VoLTE is preferred: while IMS is registered, and while an attempt to
/// register lasts. The modem only registers with VoLTE preferred; without
/// IMS, a network may refuse packet service. See DESIGN.md, VoLTE.
struct Attempt {
    registered: bool,
    give_up_at: Option<Instant>,
    retry_at: Option<Instant>,
    pause: Duration,
}

impl Attempt {
    fn start() -> Self {
        Self {
            registered: false,
            give_up_at: Some(Instant::now() + ATTEMPT),
            retry_at: None,
            pause: FIRST_PAUSE,
        }
    }

    fn wake(&self) -> Option<Instant> {
        self.give_up_at.or(self.retry_at)
    }

    /// What to report when registration comes or goes, if anything changed.
    fn registered(&mut self, up: bool) -> Option<Registration> {
        if up == self.registered {
            return None;
        }
        self.registered = up;
        if up {
            log("registered");
            self.give_up_at = None;
            self.retry_at = None;
            self.pause = FIRST_PAUSE;
            Some(Registration::Registered)
        } else {
            log("registration lost");
            self.give_up_at = Some(Instant::now() + ATTEMPT);
            Some(Registration::Registering)
        }
    }

    /// Ends the attempt, if one runs: calls go over 2G/3G until the next.
    fn give_up(&mut self, now: Instant, why: &str) -> Option<Registration> {
        self.give_up_at.take()?;
        log(format!(
            "{why}; calls over 2G/3G, trying again in {} min",
            self.pause.as_secs() / 60
        ));
        self.retry_at = Some(now + self.pause);
        self.pause = (self.pause * 2).min(LAST_PAUSE);
        Some(Registration::Off)
    }

    fn timer(&mut self, now: Instant) -> Option<Registration> {
        if self.give_up_at.is_some_and(|at| at <= now) {
            let why = format!("not registered in {} minutes", ATTEMPT.as_secs() / 60);
            return self.give_up(now, &why);
        }
        if self.retry_at.is_some_and(|at| at <= now) {
            log("trying again");
            self.retry_at = None;
            self.give_up_at = Some(now + ATTEMPT);
            return Some(Registration::Registering);
        }
        None
    }
}

/// One run: everything step 1 to 3 set up, and the bearer while it runs.
struct Ims<'a> {
    config: &'a ImsConfig,
    endpoint: Endpoint,
    imsa: Client,
    nas: Client,
    _qos: Client,
    _link_local: Client,
    _link: Link,
    index: u32,
    udp: Async<UdpSocket>,
    dcm: Socket,
    /// Where the connection manager's indications go.
    modem: Option<Addr>,
    handshake: Option<ModemEnd>,
    bearer: Option<Bearer>,
    txn: u16,
}

enum Turn {
    Dcm(usize, Addr),
    Udp(usize),
    Imsa(Message),
    Nas(Message),
    Bearer(Message),
    Time,
}

impl<'a> Ims<'a> {
    async fn open(config: &'a ImsConfig, parent: &str, endpoint: Endpoint) -> Result<Self, String> {
        let qmi = |what: &'static str| move |e: Error| format!("{what}: {e}");
        let net = |e: io::Error| format!("{LINK}: {e}");

        let imsa = client(service::IMSA, "IMSA").await?;
        imsa.call(
            Message::request(IMSA_BIND).with(0x10, 0u32.to_le_bytes()),
            TIMEOUT,
        )
        .await
        .map_err(qmi("IMSA bind"))?;
        imsa.call(
            Message::request(IMSA_REGISTER_INDICATIONS).with(0x10, [1]),
            TIMEOUT,
        )
        .await
        .map_err(qmi("IMSA indications"))?;
        let nas = client(service::NAS, "NAS").await?;
        network::report_rejects(&nas)
            .await
            .map_err(qmi("NAS rejects"))?;

        // Step 1: the link-local session and its link.
        let link = Link::create(parent).map_err(net)?;
        link.0.no_ipv6_addresses(LINK).map_err(net)?;
        link.0.set_up(LINK, None).map_err(net)?;
        let address = link_local_address().map_err(net)?;
        link.0.add_address6(LINK, address, 64).map_err(net)?;
        let index = netlink::index(LINK).map_err(net)?;

        let qos = client(QOS, "QoS").await?;
        qos.call(
            Message::request(QOS_BIND_MUX)
                .with(0x10, endpoint.bytes())
                .with(0x11, [LINK_LOCAL_MUX]),
            TIMEOUT,
        )
        .await
        .map_err(qmi("QoS mux"))?;
        qos.call(
            Message::request(QOS_BIND_SUBSCRIPTION).with(0x01, SUBSCRIPTION_NONE.to_le_bytes()),
            TIMEOUT,
        )
        .await
        .map_err(qmi("QoS subscription"))?;
        let link_local = wds_on(endpoint, LINK_LOCAL_MUX, SUBSCRIPTION_NONE, IPV6).await?;
        let msg = Message::request(WDS_START_NETWORK)
            .with(0x14, LINK_LOCAL_APN.as_bytes())
            .with(0x19, [IPV6])
            .with(0x34, TECHNOLOGY_LINK_LOCAL.to_le_bytes())
            .with(0x35, [CALL_EMBEDDED]);
        start(&link_local, msg)
            .await
            .map_err(qmi("link-local session"))?;

        let udp = UdpSocket::bind(SocketAddrV6::new(address, 0, 0, index))
            .and_then(Async::new)
            .map_err(net)?;

        // Step 2: the connection manager.
        let dcm = Socket::new().map_err(|e| format!("QRTR: {e}"))?;
        qrtr::offer(&dcm, DCM, DCM_VERSION)
            .await
            .map_err(|e| format!("offering the connection manager: {e}"))?;
        Ok(Self {
            config,
            endpoint,
            imsa,
            nas,
            _qos: qos,
            _link_local: link_local,
            _link: link,
            index,
            udp,
            dcm,
            modem: None,
            handshake: None,
            bearer: None,
            txn: 0,
        })
    }

    /// Answers the modem and follows registration until something fails.
    async fn serve(&mut self, report: &impl Fn(Registration)) -> Result<(), String> {
        let mut dcm_buf = vec![0u8; 65536];
        let mut udp_buf = vec![0u8; 4096];
        let mut attempt = Attempt::start();
        loop {
            let turn = {
                let dcm = async {
                    self.dcm
                        .recv_from(&mut dcm_buf)
                        .await
                        .map(|(n, from)| Turn::Dcm(n, from))
                        .map_err(|e| format!("connection manager: {e}"))
                };
                let udp = async {
                    self.udp
                        .recv_from(&mut udp_buf)
                        .await
                        .map(|(n, _)| Turn::Udp(n))
                        .map_err(|e| format!("handshake: {e}"))
                };
                let imsa = async {
                    self.imsa
                        .indication()
                        .await
                        .map(Turn::Imsa)
                        .map_err(|e| format!("IMSA: {e}"))
                };
                let nas = async {
                    self.nas
                        .indication()
                        .await
                        .map(Turn::Nas)
                        .map_err(|e| format!("NAS: {e}"))
                };
                let wake = attempt.wake();
                let time = async {
                    match wake {
                        Some(at) => Timer::at(at).await,
                        None => future::pending().await,
                    };
                    Ok(Turn::Time)
                };
                let bearer = async {
                    match &self.bearer {
                        Some(b) => b
                            .wds
                            .indication()
                            .await
                            .map(Turn::Bearer)
                            .map_err(|e| format!("bearer: {e}")),
                        None => future::pending().await,
                    }
                };
                future::or(
                    future::or(future::or(dcm, udp), future::or(imsa, nas)),
                    future::or(bearer, time),
                )
                .await?
            };
            match turn {
                Turn::Dcm(n, from) => {
                    if from.port == qrtr::PORT_CTRL {
                        continue;
                    }
                    if let Ok(msg) = Message::decode(&dcm_buf[..n])
                        && msg.kind == Kind::Request
                    {
                        self.modem = Some(from);
                        self.on_request(msg, from).await?;
                    }
                }
                Turn::Udp(n) => {
                    let kind = udp_buf[..n]
                        .get(4..8)
                        .map(|k| u32::from_le_bytes(k.try_into().unwrap()));
                    if kind == Some(HELLO_REPLY) {
                        self.send_properties().await?;
                    }
                }
                Turn::Imsa(msg) => {
                    if msg.id == IMSA_REGISTRATION_CHANGED
                        && let Some(&[now]) = msg.tlv(0x01)
                        && let Some(r) = attempt.registered(now == 1)
                    {
                        report(r);
                    }
                }
                Turn::Nas(msg) => {
                    if msg.id == network::NETWORK_REJECT
                        && let Some(r) = network::parse_reject(&msg)
                        && r.domain >= 2
                        && PS_REFUSED.contains(&r.cause)
                        && let Some(r) =
                            attempt.give_up(Instant::now(), "the network refused packet service")
                    {
                        report(r);
                    }
                }
                Turn::Time => {
                    if let Some(r) = attempt.timer(Instant::now()) {
                        report(r);
                    }
                }
                Turn::Bearer(msg) => {
                    if msg.id == data::WDS_PACKET_SERVICE_STATUS
                        && matches!(data::parse_status(&msg), Some((Status::Disconnected, _)))
                        && let Some(b) = self.bearer.take()
                    {
                        log("the IMS bearer went down");
                        self.indicate(&b.token, None).await?;
                    }
                }
            }
        }
    }

    async fn reply(&self, msg: Message, to: Addr) -> Result<(), String> {
        self.dcm
            .send_to(&msg.encode(), to)
            .await
            .map_err(|e| format!("connection manager: {e}"))
    }

    async fn indicate(&mut self, token: &[u8], address: Option<IpAddr>) -> Result<(), String> {
        let Some(to) = self.modem else {
            return Ok(());
        };
        self.txn = self.txn.wrapping_add(1);
        self.reply(bearer_indication(self.txn, token, address), to)
            .await
    }

    async fn on_request(&mut self, msg: Message, from: Addr) -> Result<(), String> {
        match msg.id {
            DCM_ACTIVATE => {
                let token = msg.tlv(0x10).unwrap_or(&[0; 4]).to_vec();
                let apn = activation_apn(&msg).unwrap_or_default();
                // A failure after a success reply resets the phone: refuse in
                // the reply itself.
                let started = if apn.eq_ignore_ascii_case(BEARER_APN) {
                    self.start_bearer(token.clone()).await
                } else {
                    Err(format!("no bearer for APN {apn:?}"))
                };
                match started {
                    Ok(address) => {
                        log(format!("IMS bearer up, {address}"));
                        let reply = answer(&msg, DCM_OK)
                            .with(0x10, [DCM_BEARER])
                            .with(0x11, token.clone())
                            .with(0x12, 1u32.to_le_bytes());
                        self.reply(reply, from).await?;
                        self.indicate(&token, Some(address)).await
                    }
                    Err(e) => {
                        log(format!("refusing a bearer: {e}"));
                        self.reply(answer(&msg, DCM_FAILED), from).await
                    }
                }
            }
            DCM_DEACTIVATE => {
                let reply = answer(&msg, DCM_OK)
                    .with(0x10, msg.tlv(0x01).unwrap_or(&[DCM_BEARER]).to_vec())
                    .with(0x11, msg.tlv(0x10).unwrap_or(&[0; 4]).to_vec());
                self.reply(reply, from).await?;
                if let Some(b) = self.bearer.take() {
                    if let Err(e) = data::stop(&b.wds, b.handle).await {
                        log(format!("stopping the IMS bearer: {e}"));
                    }
                    log("IMS bearer stopped, as the modem asked");
                    self.indicate(&b.token, None).await?;
                }
                Ok(())
            }
            DCM_LINK_ADDRESS => {
                self.reply(answer(&msg, DCM_OK), from).await?;
                if let Some(end) = parse_link_address(&msg) {
                    // Step 3: the handshake.
                    let to = SocketAddrV6::new(end.address, end.port, 0, self.index);
                    self.udp
                        .send_to(&hello(end.id), to)
                        .await
                        .map_err(|e| format!("handshake: {e}"))?;
                    self.handshake = Some(end);
                }
                Ok(())
            }
            _ => self.reply(answer(&msg, DCM_OK), from).await,
        }
    }

    async fn send_properties(&self) -> Result<(), String> {
        let Some(end) = &self.handshake else {
            return Ok(());
        };
        let to = |port| SocketAddrV6::new(end.address, port, 0, self.index);
        let send = async {
            self.udp
                .send_to(&properties(end.id, self.config), to(end.port))
                .await?;
            self.udp
                .send_to(&probe(self.config), to(PROBE_PORT))
                .await?;
            self.udp.send_to(&[0], to(PROBE_PORT_2)).await
        };
        send.await.map_err(|e| format!("handshake: {e}"))?;
        log("handshake done");
        Ok(())
    }

    /// Step 4: starts the bearer; its address. IPv6 first: Iliad refuses
    /// IPv4 on its IMS profile (nx809j, 2026-11-02).
    async fn start_bearer(&mut self, token: Vec<u8>) -> Result<IpAddr, String> {
        if let Some(old) = self.bearer.take() {
            let _ = data::stop(&old.wds, old.handle).await;
        }
        let mut refused = Vec::new();
        for family in [IPV6, IPV4] {
            let wds = wds_on(self.endpoint, BEARER_MUX, SUBSCRIPTION_PRIMARY, family).await?;
            let msg = Message::request(WDS_START_NETWORK)
                .with(0x19, [family])
                .with(0x31, [BEARER_PROFILE])
                .with(0x32, [NO_PROFILE])
                .with(0x35, [CALL_EMBEDDED]);
            let reply = wds
                .request(msg, START_TIMEOUT)
                .await
                .map_err(|e| e.to_string())?;
            let handle = match (reply.result(), reply.tlv(0x01)) {
                (Ok(()), Some(&[a, b, c, d])) => u32::from_le_bytes([a, b, c, d]),
                (result, _) => {
                    let e = result
                        .err()
                        .unwrap_or(Error::Malformed("no session handle"));
                    refused.push(format!("IPv{family}: {e}, {}", data::end_reason(&reply)));
                    continue;
                }
            };
            match bearer_address(&wds, family).await {
                Ok(address) => {
                    self.bearer = Some(Bearer { wds, handle, token });
                    return Ok(address);
                }
                Err(e) => {
                    let _ = data::stop(&wds, handle).await;
                    return Err(format!("bearer address: {e}"));
                }
            }
        }
        Err(refused.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn config() -> ImsConfig {
        ImsConfig {
            manufacturer: "nubia".into(),
            model: "NX809J".into(),
            build: "B1".into(),
            release: "16".into(),
            sdk: "36".into(),
        }
    }

    #[test]
    fn reads_link_address() {
        let mut link = vec![0x90, 0x23, 0, 0, 0, 0];
        let text = b"fe80::2\0";
        link.push(text.len() as u8);
        link.extend_from_slice(text);
        let msg = Message::request(DCM_LINK_ADDRESS)
            .with(0x01, link)
            .with(0x12, [7, 0]);
        assert_eq!(
            parse_link_address(&msg),
            Some(ModemEnd {
                address: "fe80::2".parse().unwrap(),
                port: 0x2390,
                id: 7,
            })
        );
        assert_eq!(
            parse_link_address(&Message::request(DCM_LINK_ADDRESS)),
            None
        );
    }

    #[test]
    fn lays_out_properties() {
        let out = properties(9, &config());
        assert_eq!(out.len(), 16 + 5 * (8 + PROPERTY_SIZE));
        assert_eq!(
            &out[..16],
            &words(&[9, PROPERTIES, out.len() as u32, 5])[..]
        );
        // Model: key 2, length 6, then the text padded.
        let model = 16 + 8 + PROPERTY_SIZE;
        assert_eq!(&out[model..model + 8], &words(&[2, 6])[..]);
        assert_eq!(&out[model + 8..model + 14], b"NX809J");
        assert_eq!(out[model + 14], 0);
    }

    #[test]
    fn attempts_end_and_come_back() {
        let mut a = Attempt::start();
        let start = a.wake().unwrap();
        assert_eq!(a.timer(start - Duration::from_secs(1)), None);
        assert_eq!(a.timer(start), Some(Registration::Off));
        let retry = a.wake().unwrap();
        assert_eq!(retry - start, FIRST_PAUSE);
        assert_eq!(a.give_up(retry, "again"), None);
        assert_eq!(a.timer(retry), Some(Registration::Registering));
        assert_eq!(a.registered(true), Some(Registration::Registered));
        assert_eq!(a.registered(true), None);
        assert_eq!(a.wake(), None);
        assert_eq!(a.pause, FIRST_PAUSE);
        assert_eq!(a.registered(false), Some(Registration::Registering));
        assert!(a.wake().is_some());
    }

    #[test]
    fn reads_activation_apn() {
        let msg = Message::request(DCM_ACTIVATE).with(0x01, [3, b'i', b'm', b's']);
        assert_eq!(activation_apn(&msg), Some("ims"));
    }

    #[test]
    fn indicates_bearer_states() {
        let up = bearer_indication(3, &[1, 2, 3, 4], Some(Ipv4Addr::new(10, 0, 0, 1).into()));
        assert_eq!(
            (up.kind, up.txn, up.id),
            (Kind::Indication, 3, DCM_ACTIVATE)
        );
        assert_eq!(up.tlv(0x02), Some(&DCM_OK[..]));
        assert_eq!(up.tlv(0x11).unwrap()[4..], *b"\x0810.0.0.1");
        let v6 = bearer_indication(3, &[1, 2, 3, 4], Some("2001:db8::1".parse().unwrap()));
        assert_eq!(v6.tlv(0x11).unwrap()[..5], [1, 0, 0, 0, 11]);
        let down = bearer_indication(4, &[1, 2, 3, 4], None);
        assert_eq!(down.tlv(0x02), Some(&DCM_DOWN[..]));
        assert_eq!(down.tlv(0x11), None);
    }
}
