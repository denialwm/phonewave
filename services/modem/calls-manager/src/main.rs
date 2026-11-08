//! The calls manager: what handles the modem's calls for whichever calls app
//! the user set, as Android's telephony does for the dialer. It follows
//! dmail's `Calls`:
//!
//! - While a call rings here it vibrates, with hapticd playing Android's
//!   default ring pattern, 1 s on and 1 s off. A call waiting behind another
//!   does not vibrate.
//! - It posts the call as a notification of the calls app: ringing, an
//!   incoming-call banner with Decline and Answer; then, while the call goes
//!   on, a quiet one with Hang up. Answer opens the calls app on the call.
//! - While the phone is locked, a call ringing starts the calls app. Denial
//!   shows the calls app's window over the lock screen while that app's call
//!   notification stands (`lockScreen.callsApp` in Denial's settings).
//!
//! Started with the graphical session by calls-manager.service.

mod bus;
mod contacts;
mod denial;
mod words;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::pin;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use async_io::Timer;
use futures_lite::{StreamExt, future};
use zbus::Connection;
use zbus::zvariant::Value;

use bus::{CallRow, Haptics1Proxy, NotificationsProxy, SessionProxy, Voice1Proxy};
use words::Words;

/// One beat of the ring: on, then off, in ms.
const ON: u32 = 1000;
const OFF: u32 = 1000;
/// Beats per request: hapticd plays at most 30 s at a time. The next request
/// goes out as one runs out.
const BEATS: u32 = 15;

enum Event {
    /// dmail's calls changed, or dmail came or went.
    Calls,
    Invoked(u32, String),
    Token(u32, String),
    /// The vibration asked for runs out.
    RunsOut,
}

/// The notification posted for a call.
struct Posted {
    id: u32,
    /// dmail's id of the call.
    call: u8,
    /// What it says, to post again only when that changes.
    body: &'static str,
}

fn main() -> ExitCode {
    match future::block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("calls-manager: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> zbus::Result<()> {
    let system = Connection::system().await?;
    let voice = Voice1Proxy::new(&system).await?;
    let haptics = Haptics1Proxy::new(&system).await?;
    let session = Connection::session().await?;
    let notifications = NotificationsProxy::new(&session).await?;
    let graphical = match bus::graphical_session(&system).await {
        Ok(graphical) => Some(graphical),
        Err(error) => {
            eprintln!("calls-manager: finding the graphical session: {error}");
            None
        }
    };
    // Each change reads the calls afresh: dmail coming back has its own.
    let owners = voice
        .inner()
        .receive_owner_changed()
        .await?
        .map(|_| Event::Calls);
    let changes = voice.receive_calls_changed().await.map(|_| Event::Calls);
    let invoked = notifications
        .receive_action_invoked()
        .await?
        .filter_map(|signal| {
            let args = signal.args().ok()?;
            Some(Event::Invoked(*args.id(), args.action_key().clone()))
        });
    let tokens = notifications
        .receive_activation_token()
        .await?
        .filter_map(|signal| {
            let args = signal.args().ok()?;
            Some(Event::Token(*args.id(), args.activation_token().clone()))
        });
    let mut events = pin!(owners.or(changes).or(invoked).or(tokens));
    // The effect playing, and when it runs out.
    let mut playing: Option<(u64, Instant)> = None;
    let mut posted: Option<Posted> = None;
    // Denial sends an action's activation token just before the action.
    let mut token: Option<(u32, String)> = None;
    // The call ringing that was last seen.
    let mut rung: Option<u8> = None;
    let mut event = Event::Calls;
    loop {
        match event {
            Event::Calls => {
                let calls = voice.inner().get_property::<Vec<CallRow>>("Calls").await;
                let calls = calls.unwrap_or_default();
                let ringing = calls
                    .iter()
                    .find(|call| call.3 == "incoming")
                    .map(|call| call.0);
                match (ringing.is_some(), playing) {
                    (true, None) => playing = vibrate(&haptics).await,
                    (false, Some((effect, _))) => {
                        if let Err(error) = haptics.cancel(effect).await {
                            eprintln!("calls-manager: stopping the vibration: {error}");
                        }
                        playing = None;
                    }
                    _ => {}
                }
                posted = post(&notifications, &calls, posted).await;
                if ringing.is_some() && ringing != rung && locked(graphical.as_ref()).await {
                    open(&denial::calls_app(&denial::settings()), None);
                }
                rung = ringing;
            }
            Event::Token(id, value) => token = Some((id, value)),
            Event::Invoked(id, action) => {
                if let Some(call) = posted.as_ref().filter(|p| p.id == id).map(|p| p.call) {
                    let token = token.take().filter(|(of, _)| *of == id).map(|(_, t)| t);
                    act(&voice, call, &action, token).await;
                }
            }
            Event::RunsOut => playing = vibrate(&haptics).await,
        }
        let next = async { events.next().await };
        let runs_out = async {
            match playing {
                Some((_, until)) => {
                    Timer::at(until).await;
                    Some(Event::RunsOut)
                }
                None => future::pending().await,
            }
        };
        event = match future::or(next, runs_out).await {
            Some(event) => event,
            None => return Ok(()),
        };
    }
}

/// Asks hapticd for the next 30 s of the ring.
async fn vibrate(haptics: &Haptics1Proxy<'_>) -> Option<(u64, Instant)> {
    let steps: Vec<(u32, f64)> = (0..BEATS).flat_map(|_| [(ON, 1.0), (OFF, 0.0)]).collect();
    let length = Duration::from_millis(u64::from(BEATS * (ON + OFF)));
    match haptics.play_pattern("", &steps).await {
        Ok(effect) => Some((effect, Instant::now() + length)),
        Err(error) => {
            eprintln!("calls-manager: vibrating: {error}");
            None
        }
    }
}

/// Shows the call that matters, in place of the last notification: one
/// ringing here first, else the one going on. Closes it once there is none.
async fn post(
    notifications: &NotificationsProxy<'_>,
    calls: &[CallRow],
    posted: Option<Posted>,
) -> Option<Posted> {
    let shown = calls.iter().find(|call| call.3 == "incoming").or_else(|| {
        calls
            .iter()
            .find(|call| call.3 != "waiting" && call.3 != "ending")
    });
    let Some((call, number, _, state, _)) = shown else {
        if let Some(posted) = posted
            && let Err(error) = notifications.close_notification(posted.id).await
        {
            eprintln!("calls-manager: closing the call's notification: {error}");
        }
        return None;
    };
    // Read afresh each time: calls are rare, and the contacts, the language
    // and the calls app can change.
    let settings = denial::settings();
    let words = Words::new(&settings);
    let ringing = state == "incoming";
    let body = match state.as_str() {
        "incoming" => words.tr("Incoming call"),
        "dialing" | "alerting" => words.tr("Calling…"),
        "held" => words.tr("On hold"),
        _ => words.tr("Ongoing call"),
    };
    let same = posted.as_ref().filter(|p| p.call == *call);
    if same.is_some_and(|p| p.body == body) {
        return posted;
    }
    let name = match contacts::name_of(number) {
        Some(name) => name,
        None if number.is_empty() => words.tr("Unknown caller").into(),
        None => number.clone(),
    };
    let app = denial::calls_app(&settings);
    let mut hints = HashMap::from([
        ("desktop-entry", Value::from(app.as_str())),
        ("x-denial-clearable", Value::from(false)),
    ]);
    let actions: &[&str] = if ringing {
        hints.insert("category", Value::from("call.incoming"));
        hints.insert("urgency", Value::from(2u8));
        // A new ring alerts; the same call again does not.
        hints.insert("x-denial-alert", Value::from(same.is_none()));
        &["decline", words.tr("Decline"), "answer", words.tr("Answer")]
    } else {
        hints.insert("category", Value::from("call"));
        hints.insert("resident", Value::from(true));
        hints.insert("x-denial-alert", Value::from(false));
        &["hangup", words.tr("Hang up")]
    };
    let replaces = posted.as_ref().map_or(0, |p| p.id);
    let name_of_app = desktop_entry(&app).map(|entry| entry.0).unwrap_or_default();
    match notifications
        .notify(&name_of_app, replaces, "", &name, body, actions, hints, -1)
        .await
    {
        Ok(id) => Some(Posted {
            id,
            call: *call,
            body,
        }),
        Err(error) => {
            eprintln!("calls-manager: posting the call: {error}");
            posted
        }
    }
}

/// Runs a notification's action on call `call`.
async fn act(voice: &Voice1Proxy<'_>, call: u8, action: &str, token: Option<String>) {
    let done = match action {
        "answer" => {
            let done = voice.answer(call).await;
            if done.is_ok() {
                open(&denial::calls_app(&denial::settings()), token);
            }
            done
        }
        "decline" | "hangup" => voice.hangup(call).await,
        _ => return,
    };
    if let Err(error) = done {
        eprintln!("calls-manager: {action}: {error}");
    }
}

/// Whether Denial holds the session locked.
async fn locked(session: Option<&SessionProxy<'_>>) -> bool {
    let Some(session) = session else {
        return false;
    };
    session.locked_hint().await.unwrap_or_else(|error| {
        eprintln!("calls-manager: reading the lock: {error}");
        false
    })
}

/// Starts the app desktop entry `id` names, or raises it if it runs, with the
/// activation token.
fn open(id: &str, token: Option<String>) {
    let Some((_, exec)) = desktop_entry(id) else {
        return eprintln!("calls-manager: no desktop entry {id}");
    };
    let Some((program, args)) = exec.split_first() else {
        return;
    };
    let mut command = Command::new(program);
    command.args(args);
    if let Some(token) = token {
        command.env("XDG_ACTIVATION_TOKEN", token);
    }
    match command.spawn() {
        // Reaped on its own thread so it never lingers as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(error) => eprintln!("calls-manager: {program}: {error}"),
    }
}

/// The name and the command of desktop entry `id`, from the first data
/// folder that has it. Field codes such as `%u` are left out.
fn desktop_entry(id: &str) -> Option<(String, Vec<String>)> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")));
    let dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    let text = home
        .into_iter()
        .chain(
            dirs.split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .find_map(|dir| {
            std::fs::read_to_string(dir.join(format!("applications/{id}.desktop"))).ok()
        })?;
    let mut in_entry = false;
    let (mut name, mut exec) = (None, None);
    for line in text.lines() {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some(value) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| value.to_owned());
        } else if in_entry && let Some(value) = line.strip_prefix("Exec=") {
            exec.get_or_insert_with(|| {
                value
                    .split_whitespace()
                    .filter(|arg| !arg.starts_with('%'))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });
        }
    }
    Some((name.unwrap_or_default(), exec?))
}
