//! steinerd: the sensor service.

use std::process::ExitCode;

use async_executor::LocalExecutor;
use async_signal::{Signal, Signals};
use futures_lite::future;
use steinerd::bus::Bus;
use steinerd::daemon::Daemon;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("steinerd: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let signals = Signals::new([Signal::Term, Signal::Int]).map_err(|e| format!("signals: {e}"))?;
    let (commands_tx, commands) = async_channel::unbounded();
    let ex = LocalExecutor::new();
    future::block_on(ex.run(async {
        let bus = Bus::serve(&ex, commands_tx)
            .await
            .map_err(|e| format!("D-Bus: {e}"))?;
        eprintln!("steinerd: on the system bus");
        Daemon {
            bus,
            commands,
            signals,
        }
        .run()
        .await
    }))
}
