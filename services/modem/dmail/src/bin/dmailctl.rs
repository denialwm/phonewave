//! Talks to the modem directly, for bring-up and debugging.

use std::error::Error;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_io::Timer;
use async_signal::{Signal, Signals};

use dmail::config::{self, DataConfig, Device};
use dmail::data::{self, Endpoint, Status};
use dmail::netlink::{self, Netlink};
use dmail::qmi::{self, service};
use dmail::qrtr::{self, Event, Lookup};
use dmail::sim::{self, Pin};
use dmail::sms::pdu;
use dmail::sms::wms::{self, Incoming};
use dmail::{location as loc, modem, network};
use futures_lite::{StreamExt, future};

const USAGE: &str = "usage: dmailctl services
       dmailctl status
       dmailctl number
       dmailctl unlock
       dmailctl sms routes
       dmailctl sms listen
       dmailctl sms send NUMBER|self TEXT
       dmailctl data up APN
       dmailctl watch
       dmailctl radio cycle
       dmailctl volte on|off
       dmailctl location [raw]
       dmailctl xtra [FILE]";

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    // Die quietly when a pipe reader such as `grep -q` leaves early.
    // SAFETY: no other thread exists yet.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["services"] => future::block_on(services()),
        ["status"] => future::block_on(status()),
        ["number"] => future::block_on(number()),
        ["unlock"] => future::block_on(unlock()),
        ["watch"] => future::block_on(watch()),
        ["radio", "cycle"] => future::block_on(radio_cycle()),
        ["volte", "on"] => future::block_on(volte(true)),
        ["volte", "off"] => future::block_on(volte(false)),
        ["sms", "routes"] => future::block_on(routes()),
        ["sms", "listen"] => future::block_on(listen()),
        ["sms", "send", number, text] => future::block_on(send(number, text)),
        ["location"] | ["location", "raw"] => Signals::new([Signal::Term, Signal::Int])
            .map_err(Into::into)
            .and_then(|mut signals| future::block_on(location(&mut signals, args.len() == 2))),
        ["xtra"] => future::block_on(xtra(None)),
        ["xtra", file] => future::block_on(xtra(Some(file))),
        ["data", "up", apn] => Signals::new([Signal::Term, Signal::Int])
            .map_err(Into::into)
            .and_then(|mut signals| future::block_on(data_up(&mut signals, apn))),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dmailctl: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn client(id: u32) -> Result<qmi::Client> {
    let name = service::name(id).unwrap_or("?");
    let found = qrtr::find(id)
        .await?
        .ok_or_else(|| format!("no {name} service; is the modem running?"))?;
    Ok(qmi::Client::new(found.addr)?)
}

/// Lists what the name service knows, modem services first by name.
async fn services() -> Result {
    let lookup = Lookup::new(0).await?;
    let mut found = Vec::new();
    loop {
        match lookup.next().await? {
            Event::Up(s) => found.push(s),
            Event::Down(_) => {}
            Event::Listed => break,
        }
    }
    found.sort_by_key(|s| (s.addr.node, s.id, s.instance));
    println!(
        "{:>4} {:>6} {:>7} {:>3} {:>8}  name",
        "node", "port", "service", "ver", "instance"
    );
    for s in found {
        let name = service::name(s.id).unwrap_or("");
        println!(
            "{:>4} {:>6} {:>7} {:>3} {:>8}  {name}",
            s.addr.node, s.addr.port, s.id, s.version, s.instance
        );
    }
    Ok(())
}

fn app_state(state: u8) -> &'static str {
    match state {
        1 => "detected",
        2 => "PIN required",
        3 => "PUK required",
        4 => "personalization check",
        5 => "PIN blocked",
        6 => "illegal",
        7 => "ready",
        _ => "unknown",
    }
}

fn perso(usim: &sim::App) -> String {
    let state = match usim.perso_state {
        1 => "in progress",
        2 => "ready",
        3 => "code required",
        4 => "PUK required",
        5 => "blocked",
        _ => "unknown",
    };
    let feature = match usim.perso_feature {
        0 => "network",
        1 => "network subset",
        2 => "service provider",
        3 => "corporate",
        4 => "SIM",
        _ => "other",
    };
    format!("{state} ({feature} {})", usim.perso_feature)
}

fn registration(state: u8) -> &'static str {
    match state {
        0 => "not registered",
        1 => "registered",
        2 => "searching",
        3 => "denied",
        _ => "unknown",
    }
}

/// Reads only; changes nothing on the modem.
async fn status() -> Result {
    let dms = client(service::DMS).await?;
    println!(
        "modem: {}",
        modem::mode_name(modem::operating_mode(&dms).await?)
    );
    for subscription in [1, 2] {
        println!("imei {subscription}: {}", imei(subscription).await?);
    }

    let uim = client(service::UIM).await?;
    let cards = sim::card_status(&uim).await?;
    let session = |slot: Option<u8>| slot.map_or("none".into(), |s| format!("sim {s}"));
    println!(
        "sessions: primary {}, secondary {}",
        session(cards.primary),
        session(cards.secondary)
    );
    for (i, card) in cards.cards.iter().enumerate() {
        let slot = i + 1;
        match (card.state, card.usim()) {
            (sim::CARD_PRESENT, Some(usim)) => println!(
                "sim {slot}: usim {}, personalization {}, PIN {:?} ({} tries, PUK {})",
                app_state(usim.state),
                perso(usim),
                usim.pin1,
                usim.pin1_retries,
                usim.puk1_retries
            ),
            (sim::CARD_PRESENT, None) => println!("sim {slot}: present, no usim"),
            // No answer to reset: an empty slot.
            _ if card.error == 3 => println!("sim {slot}: empty"),
            (0, _) => println!("sim {slot}: absent"),
            (_, _) => println!("sim {slot}: error {}", card.error),
        }
    }

    let nas = client(service::NAS).await?;
    let s = network::serving(&nas).await?;
    let technology = network::shown_technology(&nas, &s).await?;
    println!("network: {}", network_line(&s, technology));
    let (usage, voice) = network::voice_preference(&nas).await?;
    println!(
        "usage {usage:?} (1 voice, 2 data centric), voice domain {voice:?} (2 cs, 3 ps preferred)"
    );
    if let Some((mcc, mnc, name)) = &s.plmn {
        println!("operator: {name} ({mcc}-{mnc:02})");
    }
    if let Some(roaming) = s.roaming {
        println!("roaming: {}", if roaming { "yes" } else { "no" });
    }
    match network::signal(&nas).await? {
        Some(percent) => println!("signal: {percent}%"),
        None => println!("signal: none"),
    }
    Ok(())
}

fn network_line(s: &network::Serving, technology: network::Technology) -> String {
    format!(
        "{}, {}, packet {}",
        registration(s.registration),
        technology.as_str(),
        if s.packet_attached() {
            "attached"
        } else {
            "detached"
        }
    )
}

/// Prints registration changes and every network reject, with the time.
async fn watch() -> Result {
    let nas = client(service::NAS).await?;
    network::report_rejects(&nas).await?;
    let stamp = || {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("{:02}:{:02}:{:02}Z", t / 3600 % 24, t / 60 % 60, t % 60)
    };
    let mut last = String::new();
    loop {
        let s = network::serving(&nas).await?;
        let line = network_line(&s, network::shown_technology(&nas, &s).await?);
        if line != last {
            let reject = network::sim_reject_state(&nas).await?;
            println!("{} {line}, sim reject state {reject:?}", stamp());
            last = line;
        }
        let msg = nas.indication().await?;
        if let Some(r) = network::parse_reject(&msg).filter(|_| msg.id == network::NETWORK_REJECT) {
            println!(
                "{} reject: radio {} domain {} cause {} ({})",
                stamp(),
                r.radio,
                r.domain,
                r.cause,
                r.cause_name()
            );
        }
    }
}

/// VoLTE on: voice centric, voice over LTE preferred. Off: data centric,
/// voice over 2G/3G preferred. Lasts until the modem restarts.
async fn volte(on: bool) -> Result {
    let nas = client(service::NAS).await?;
    let (usage, voice) = if on {
        (network::VOICE_CENTRIC, network::VOICE_PS_PREFERRED)
    } else {
        (network::DATA_CENTRIC, network::VOICE_CS_PREFERRED)
    };
    network::set_voice_preference(&nas, usage, voice).await?;
    let (usage, voice) = network::voice_preference(&nas).await?;
    println!("usage {usage:?}, voice domain {voice:?}");
    Ok(())
}

/// Radio off and on: clears the modem's memory of a rejected domain.
async fn radio_cycle() -> Result {
    let dms = client(service::DMS).await?;
    modem::set_low_power(&dms).await?;
    println!(
        "modem: {}",
        modem::mode_name(modem::operating_mode(&dms).await?)
    );
    Timer::after(Duration::from_secs(2)).await;
    modem::set_online(&dms).await?;
    println!(
        "modem: {}",
        modem::mode_name(modem::operating_mode(&dms).await?)
    );
    Ok(())
}

/// "valid" or "missing" for one subscription's IMEI, never the value.
async fn imei(subscription: u32) -> Result<&'static str> {
    let dms = client(service::DMS).await?;
    modem::bind_subscription(&dms, subscription).await?;
    Ok(if modem::imei_valid(&dms).await? {
        "valid"
    } else {
        "missing"
    })
}

/// Puts the modem online, opens the SIM's provisioning session and, if the
/// SIM wants it, asks for PIN1 once. The PIN is never echoed, logged or kept.
/// Refuses to go online unless both IMEIs are valid.
async fn unlock() -> Result {
    for subscription in [1, 2] {
        if imei(subscription).await? != "valid" {
            return Err(format!("imei {subscription} missing; not going online").into());
        }
    }
    let dms = client(service::DMS).await?;
    if modem::operating_mode(&dms).await? != modem::ONLINE {
        modem::set_online(&dms).await?;
        println!("modem: online");
    }

    let uim = client(service::UIM).await?;
    let (slot, usim) = first_usim(&uim).await?;
    sim::activate(&uim, slot, &usim.aid).await?;
    // The PIN state settles a moment after the session opens.
    let mut usim = usim;
    for _ in 0..10 {
        if usim.pin1 != Pin::NotInitialized {
            break;
        }
        Timer::after(Duration::from_secs(1)).await;
        usim = first_usim(&uim).await?.1;
    }

    match usim.pin1 {
        Pin::Required => {}
        Pin::Verified | Pin::Disabled => {
            println!("sim {slot}: no PIN needed");
            return Ok(());
        }
        other => return Err(format!("sim {slot}: PIN {other:?}, not unlocking").into()),
    }
    let mut pin = read_secret(&format!(
        "PIN for SIM {slot} ({} tries left): ",
        usim.pin1_retries
    ))?;
    if !(4..=8).contains(&pin.len()) || !pin.iter().all(u8::is_ascii_digit) {
        pin.fill(0);
        return Err("a PIN is 4 to 8 digits; nothing sent".into());
    }
    let verified = sim::verify_pin1(&uim, &pin).await;
    pin.fill(0);
    let (outcome, retries) = verified?;
    match outcome {
        Ok(()) => println!("sim {slot}: unlocked"),
        Err(e) => println!("sim {slot}: not unlocked ({e})"),
    }
    if let Some((pin, puk)) = retries {
        println!("tries left: PIN {pin}, PUK {puk}");
    }
    Ok(())
}

/// The first present card's slot (1-based) and usim.
async fn first_usim(uim: &qmi::Client) -> Result<(u8, sim::App)> {
    let found = sim::card_status(uim)
        .await?
        .cards
        .iter()
        .zip(1u8..)
        .filter(|(card, _)| card.state == sim::CARD_PRESENT)
        .find_map(|(card, slot)| Some((slot, card.usim()?.clone())));
    Ok(found.ok_or("no SIM with a usim")?)
}

/// Reads one line from the terminal with echo off.
fn read_secret(prompt: &str) -> io::Result<Vec<u8>> {
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("no terminal to ask on ({e}); run with ssh -t"),
            )
        })?;
    tty.write_all(prompt.as_bytes())?;
    let fd = tty.as_raw_fd();
    // SAFETY: termios is plain data and fd is an open terminal.
    let saved = unsafe {
        let mut term = std::mem::zeroed::<libc::termios>();
        if libc::tcgetattr(fd, &mut term) != 0 {
            return Err(io::Error::last_os_error());
        }
        term
    };
    let mut quiet = saved;
    quiet.c_lflag &= !libc::ECHO;
    quiet.c_lflag |= libc::ECHONL;
    // SAFETY: as above.
    unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &quiet) };
    let mut line = Vec::new();
    let mut byte = [0u8];
    let read = loop {
        match tty.read(&mut byte) {
            Ok(1) if byte[0] != b'\n' => line.push(byte[0]),
            Ok(_) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    // SAFETY: as above.
    unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &saved) };
    read.map(|()| line)
}

async fn routes() -> Result {
    let wms = client(service::WMS).await?;
    let action = |a: u8| match a {
        0 => "discard",
        1 => "store and notify",
        2 => "transfer only",
        3 => "transfer and ack",
        _ => "unknown",
    };
    let (routes, reports) = wms::routes(&wms).await?;
    for r in routes {
        let storage = match r.storage {
            0 => "sim",
            1 => "modem",
            _ => "none",
        };
        println!(
            "type {} class {}: {}, {storage}",
            r.kind,
            r.class,
            action(r.action)
        );
    }
    println!(
        "status reports: {}",
        if reports {
            "transferred"
        } else {
            "not transferred"
        }
    );
    Ok(())
}

fn show(tpdu: &[u8]) {
    match pdu::decode(tpdu) {
        Ok(pdu::Tpdu::Deliver(m)) => {
            let part = m
                .part
                .map(|p| format!(" [part {}/{} ref {}]", p.sequence, p.total, p.reference))
                .unwrap_or_default();
            let time = m.time.map(|t| t.to_string()).unwrap_or_else(|| "?".into());
            println!("from {} at {time}{part}: {}", m.sender, m.text);
        }
        Ok(pdu::Tpdu::StatusReport(r)) => println!(
            "status report for reference {}: status {:#04x}",
            r.reference, r.status
        ),
        Err(e) => println!("undecodable message ({e:?}), {} bytes", tpdu.len()),
    }
}

/// Prints new messages as they arrive, acking the ones the network hands
/// over. Stored messages are read, never deleted.
async fn listen() -> Result {
    let wms = client(service::WMS).await?;
    wms::report_new_messages(&wms).await?;
    println!("listening");
    loop {
        let msg = wms.indication().await?;
        if msg.id != wms::EVENT_REPORT {
            continue;
        }
        for incoming in wms::parse_event(&msg) {
            match incoming {
                Incoming::Transferred {
                    ack,
                    transaction,
                    tpdu,
                    on_ims,
                    ..
                } => {
                    show(&tpdu);
                    if ack {
                        wms::ack(&wms, transaction, on_ims).await?;
                        println!("acked");
                    }
                }
                Incoming::Stored { storage, index } => {
                    let (_, data) = wms::raw_read(&wms, storage, index).await?;
                    println!("stored at {storage}/{index}");
                    show(pdu::skip_smsc(&data).map_err(|e| format!("{e:?}"))?);
                }
            }
        }
    }
}

/// The SIM's own number.
async fn own_number() -> Result<String> {
    let dms = client(service::DMS).await?;
    Ok(modem::msisdn(&dms)
        .await?
        .ok_or("the SIM holds no own number")?)
}

async fn number() -> Result {
    println!("{}", own_number().await?);
    Ok(())
}

/// Sends `text` to `number`, or to the SIM's own number for "self".
async fn send(number: &str, text: &str) -> Result {
    let own;
    let number = match number {
        "self" => {
            own = own_number().await?;
            own.as_str()
        }
        n => n,
    };
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.subsec_nanos();
    let parts =
        pdu::encode_submit(number, text, nanos as u8, false).map_err(|e| format!("{e:?}"))?;
    let wms = client(service::WMS).await?;
    for (part, n) in parts.iter().zip(1..) {
        match wms::raw_send(&wms, part).await? {
            (Ok(reference), _) => println!("part {n}/{}: sent, reference {reference}", parts.len()),
            (Err(e), cause) => {
                return Err(format!("part {n}/{}: {e}, cause {cause:?}", parts.len()).into());
            }
        }
    }
    Ok(())
}

const LINK: &str = "rmnet_data0";
const MUX_ID: u8 = 1;
const METRIC: u32 = 10000;

/// Brings a data session up on `apn` and holds it until a signal or the
/// network ends it, on the data path the phone's device file names. The
/// link goes away with the session.
async fn data_up(signals: &mut Signals, apn: &str) -> Result {
    let path = config::device_file().ok_or("this phone has no dmail device file")?;
    let device = Device::load(&path)?;
    let cfg = &device.data;
    let endpoint = device.endpoint()?;
    let nas = client(service::NAS).await?;
    let serving = network::serving(&nas).await?;
    if !serving.packet_attached() {
        return Err("packet service not attached".into());
    }
    let dpm = client(service::DPM).await?;
    data::open_port(&dpm, endpoint, cfg.dpm.rx, cfg.dpm.tx).await?;
    let wda = client(service::WDA).await?;
    data::set_format(&wda, endpoint, cfg.dl_datagrams, cfg.dl_datagram_size).await?;

    let net = Netlink::new()?;
    // The link is dmail's alone: whatever is left of it is stale.
    if netlink::index(LINK).is_ok() {
        net.delete_link(LINK)?;
    }
    net.add_rmnet(&cfg.ipa_netdev, LINK, MUX_ID.into(), netlink::RMNET_FLAGS)?;
    let held = hold_session(signals, &net, cfg, endpoint, apn).await;
    net.delete_link(LINK)?;
    println!("{LINK} removed");
    held
}

async fn hold_session(
    signals: &mut Signals,
    net: &Netlink,
    cfg: &DataConfig,
    endpoint: Endpoint,
    apn: &str,
) -> Result {
    net.set_up(&cfg.ipa_netdev, None)?;
    let wds = client(service::WDS).await?;
    data::bind_mux(&wds, endpoint, MUX_ID).await?;
    data::prefer_ipv4(&wds).await?;
    let handle = match data::start(&wds, apn).await? {
        (Ok(handle), _) => handle,
        (Err(e), why) => return Err(format!("session refused: {e}, {why:?}").into()),
    };
    let held = async {
        let s = data::settings(&wds).await?;
        net.set_up(LINK, Some(s.mtu.unwrap_or(cfg.mtu_max).min(cfg.mtu_max)))?;
        net.add_address(LINK, s.address, s.prefix)?;
        net.add_default_route(LINK, METRIC)?;
        println!(
            "connected on {apn}: {}/{} mtu {:?} dns {:?}",
            s.address, s.prefix, s.mtu, s.dns
        );
        let ended = async {
            loop {
                let msg = wds.indication().await?;
                if msg.id != data::WDS_PACKET_SERVICE_STATUS {
                    continue;
                }
                match data::parse_status(&msg) {
                    Some((Status::Disconnected, why)) => {
                        return Ok::<_, Box<dyn Error>>(format!("network ended it: {why:?}"));
                    }
                    Some((status, _)) => println!("status: {status:?}"),
                    None => {}
                }
            }
        };
        let stopped = async {
            let signal = signals.next().await.ok_or("signal stream ended")??;
            Ok(format!("signal {signal:?}"))
        };
        let why = future::or(ended, stopped).await?;
        println!("session over: {why}");
        Ok::<_, Box<dyn Error>>(())
    }
    .await;
    if let Err(e) = data::stop(&wds, handle).await {
        println!("stop: {e}");
    }
    held
}

/// How long `location` runs at most.
const LOCATION_LIMIT: Duration = Duration::from_secs(15 * 60);

/// Unlocks the GNSS engine, follows a session until Ctrl-C or 15 minutes,
/// then puts the engine lock back as it was. `raw` prints every indication's
/// TLVs as well.
async fn location(signals: &mut Signals, raw: bool) -> Result {
    let ctl = client(service::LOC).await?;
    let lock = loc::engine_lock(&ctl).await?;
    let mode = match loc::operation_mode(&ctl).await {
        Ok(mode) => loc::mode_name(mode),
        Err(e) => {
            eprintln!("operation mode: {e}");
            "?"
        }
    };
    println!("engine lock {}, mode {mode}", loc::lock_name(lock));
    if lock != loc::LOCK_NONE {
        loc::set_engine_lock(&ctl, loc::LOCK_NONE).await?;
        println!("engine unlocked");
    }
    let followed = follow_location(signals, &ctl, raw).await;
    if lock != loc::LOCK_NONE {
        loc::set_engine_lock(&ctl, lock).await?;
        println!("engine lock back to {}", loc::lock_name(lock));
    }
    followed
}

async fn follow_location(signals: &mut Signals, ctl: &qmi::Client, raw: bool) -> Result {
    const SESSION: u8 = 1;
    let session = client(service::LOC).await?;
    loc::register(&session).await?;
    match loc::inject_time(ctl).await {
        Ok(true) => println!("time injected"),
        Ok(false) => println!("system clock not synchronized: no time injected"),
        Err(e) => eprintln!("injecting time: {e}"),
    }
    loc::start(&session, SESSION).await?;
    let started = Instant::now();
    println!("session started");
    let reports = async {
        let mut first_fix = None;
        let mut tracked = (0, 0);
        loop {
            let msg = session.indication().await?;
            let t = started.elapsed().as_secs_f32();
            if raw {
                print!("{t:7.1}s ind {:#06x}:", msg.id);
                for (k, v) in msg.tlvs() {
                    let head: String = v.iter().take(24).map(|b| format!("{b:02x}")).collect();
                    print!(" [{k:#04x} {}B {head}]", v.len());
                }
                println!();
            }
            match msg.id {
                loc::POSITION_REPORT => {
                    let Some(r) = loc::parse_report(&msg) else {
                        continue;
                    };
                    if r.is_fix() && first_fix.is_none() {
                        first_fix = Some(t);
                        println!("first fix after {t:.1} s");
                    }
                    let opt = |v: Option<f32>, unit: &str| {
                        v.map_or("-".into(), |v| format!("{v:.1}{unit}"))
                    };
                    println!(
                        "{t:7.1}s {:<9} {:.6} {:.6} ±{} alt {} speed {} used {} tracked {}/{}",
                        loc::status_name(r.status),
                        r.latitude.unwrap_or(f64::NAN),
                        r.longitude.unwrap_or(f64::NAN),
                        opt(r.accuracy, " m"),
                        opt(r.altitude, " m"),
                        opt(r.speed, " m/s"),
                        r.used,
                        tracked.1,
                        tracked.0,
                    );
                }
                loc::SATELLITES => {
                    if let Some(seen) = loc::parse_satellites(&msg) {
                        tracked = seen;
                    }
                }
                loc::ENGINE_STATE | loc::FIX_SESSION_STATE => {
                    let what = if msg.id == loc::ENGINE_STATE {
                        "engine"
                    } else {
                        "fix session"
                    };
                    let state = msg.tlv(0x01).map(|v| v.first().copied().unwrap_or(0));
                    println!("{t:7.1}s {what} state {state:?}");
                }
                loc::INJECT_TIME_WANTED => println!("{t:7.1}s the engine asks for the time"),
                _ => {}
            }
        }
    };
    let ended = future::or(
        future::or(reports, async {
            signals.next().await;
            Ok::<_, Box<dyn Error>>(())
        }),
        async {
            Timer::after(LOCATION_LIMIT).await;
            println!("{} minutes up", LOCATION_LIMIT.as_secs() / 60);
            Ok(())
        },
    )
    .await;
    loc::stop(&session, SESSION).await?;
    println!("session stopped");
    ended
}

/// Prints the span the engine's predicted orbits cover; with a file, injects
/// it first.
async fn xtra(file: Option<&str>) -> Result {
    let ctl = client(service::LOC).await?;
    if let Some(file) = file {
        let data = std::fs::read(file)?;
        let start = Instant::now();
        loc::inject_xtra(&ctl, &data).await?;
        let took = start.elapsed().as_secs_f32();
        println!("injected {} bytes in {took:.1} s", data.len());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match loc::xtra_validity(&ctl).await? {
        Some((start, hours)) => {
            let ago = now.saturating_sub(start) as f64 / 3600.0;
            println!("predicted orbits: {hours} h from {ago:.1} h ago");
        }
        None => println!("predicted orbits: none"),
    }
    Ok(())
}
