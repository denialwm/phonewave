//! callaudio: puts the sound card on its "Voice Call" UCM verb while the
//! modem has a call, and back on its previous profile afterwards. The calls
//! come from dmail (`org.dmail.Voice1.Calls`, system bus); the card's
//! profile is PipeWire's, so callaudio runs in the user's session.

mod native;
mod service;

use std::process::ExitCode;

use async_signal::{Signal, Signals};
use futures_lite::{StreamExt, future};
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::InterfaceName;
use zbus::object_server::InterfaceRef;
use zbus::proxy::CacheProperties;

use service::{CallAudio, VERB};

const NAME: &str = "org.callaudio";
const PATH: &str = "/org/callaudio";
const DMAIL: &str = "org.dmail";
const VOICE: &str = "org.dmail.Voice1";

type Call = (u8, String, String, String, i64);

/// Speech flows from dialling on: the network plays the ringing tone. An
/// incoming call rings through the speakers' normal profile until answered.
fn needs_audio(calls: &[Call]) -> bool {
    calls
        .iter()
        .any(|call| matches!(call.3.as_str(), "dialing" | "alerting" | "active" | "held"))
}

async fn calls(properties: &PropertiesProxy<'_>) -> Vec<Call> {
    let voice = InterfaceName::from_static_str_unchecked(VOICE);
    match properties.get(voice, "Calls").await {
        Ok(value) => Vec::<Call>::try_from(value).unwrap_or_default(),
        // dmail is not running: no calls.
        Err(_) => Vec::new(),
    }
}

async fn follow(audio: &InterfaceRef<CallAudio>, call: bool) {
    let changed = audio.get_mut().await.follow(call);
    if changed {
        let emitter = audio.signal_emitter();
        let audio = audio.get().await;
        let _ = audio.active_changed(emitter).await;
        let _ = audio.muted_changed(emitter).await;
    }
}

enum Event {
    Changed,
    Stop,
}

async fn run(card: native::Card) -> zbus::Result<()> {
    let session = zbus::connection::Builder::session()?
        .name(NAME)?
        .serve_at(PATH, CallAudio::new(card))?
        .build()
        .await?;
    let audio = session
        .object_server()
        .interface::<_, CallAudio>(PATH)
        .await?;

    let system = zbus::Connection::system().await?;
    let properties = PropertiesProxy::builder(&system)
        .destination(DMAIL)?
        .path("/org/dmail")?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let mut changes = properties.receive_properties_changed().await?;
    let mut owners = DBusProxy::new(&system)
        .await?
        .receive_name_owner_changed_with_args(&[(0, DMAIL)])
        .await?;
    let mut signals = Signals::new([Signal::Term, Signal::Int])?;

    loop {
        follow(&audio, needs_audio(&calls(&properties).await)).await;
        let event = future::or(
            async {
                match changes.next().await {
                    Some(_) => Event::Changed,
                    None => Event::Stop,
                }
            },
            future::or(
                async {
                    match owners.next().await {
                        Some(_) => Event::Changed,
                        None => Event::Stop,
                    }
                },
                async {
                    signals.next().await;
                    Event::Stop
                },
            ),
        )
        .await;
        if let Event::Stop = event {
            break;
        }
    }
    follow(&audio, false).await;
    Ok(())
}

fn main() -> ExitCode {
    let card = match native::find_card(VERB) {
        Ok(card) => card,
        Err(e) => {
            // Nothing to do on this device.
            eprintln!("no sound card has a \"{VERB}\" UCM verb: {e}");
            return ExitCode::SUCCESS;
        }
    };
    eprintln!(
        "card {}: volume {:?}, mute {:?}",
        card.index, card.volume, card.mute
    );
    match async_io::block_on(run(card)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("callaudio: {e}");
            ExitCode::FAILURE
        }
    }
}
