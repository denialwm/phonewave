//! dmail: the modem service. See DESIGN.md.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use async_executor::LocalExecutor;
use async_signal::{Signal, Signals};
use dmail::bus::Bus;
use dmail::config::{self, Device, Settings};
use dmail::daemon::Daemon;
use dmail::sms::store::Store;
use futures_lite::future;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dmail: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let device = Device::load(Path::new(config::DEVICE_FILE))?;
    let state_dir = PathBuf::from(config::STATE_DIR);
    let store =
        Store::open(&state_dir.join("messages.db")).map_err(|e| format!("message store: {e}"))?;
    let unsent = store
        .fail_unsent()
        .map_err(|e| format!("message store: {e}"))?;
    if !unsent.is_empty() {
        eprintln!(
            "dmail: {} messages were cut off while sending",
            unsent.len()
        );
    }
    let store = Arc::new(Mutex::new(store));
    let settings = Settings::load(&state_dir);
    let signals = Signals::new([Signal::Term, Signal::Int]).map_err(|e| format!("signals: {e}"))?;
    let (commands_tx, commands) = async_channel::unbounded();

    let ex = LocalExecutor::new();
    future::block_on(ex.run(async {
        let bus = Bus::serve(&ex, commands_tx, store.clone(), settings)
            .await
            .map_err(|e| format!("D-Bus: {e}"))?;
        eprintln!("dmail: on the system bus");
        Daemon {
            ex: &ex,
            bus,
            device,
            store,
            state_dir,
            commands,
            signals,
            settings,
        }
        .run()
        .await
    }))
}
