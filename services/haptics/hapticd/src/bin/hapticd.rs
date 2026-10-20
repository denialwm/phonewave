use futures_lite::{StreamExt, future};
use hapticd::{
    BUS_NAME, Config, OBJECT_PATH,
    engine::{self, Command},
    service::{Owners, Service},
};
use std::{error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    let mut session = false;
    let mut path = "/etc/hapticd/config.json".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session" => session = true,
            "--config" => path = args.next().ok_or("--config needs a path")?,
            "--version" => {
                println!("hapticd {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" => {
                println!(
                    "hapticd [--config PATH] [--session]\nA Linux haptics service. Uses the system bus by default."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let config = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Config>(&text)?,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound && path == "/etc/hapticd/config.json" =>
        {
            Config::default()
        }
        Err(e) => return Err(e.into()),
    };
    let (worker, thread) = engine::spawn(config.clone())?;
    let result: Result<(), Box<dyn Error>> = future::block_on(async {
        let tracked_owners = Owners::default();
        let builder = if session {
            zbus::connection::Builder::session()?
        } else {
            zbus::connection::Builder::system()?
        };
        let connection = builder
            .method_timeout(std::time::Duration::from_secs(2))
            .serve_at(
                OBJECT_PATH,
                Service {
                    worker: worker.clone(),
                    config,
                    session,
                    owners: tracked_owners.clone(),
                },
            )?
            .build()
            .await?;
        let bus = zbus::fdo::DBusProxy::new(&connection).await?;
        let mut owners = bus.receive_name_owner_changed().await?;
        let mut signals =
            async_signal::Signals::new([async_signal::Signal::Term, async_signal::Signal::Int])?;
        connection.request_name(BUS_NAME).await?;
        eprintln!(
            "hapticd: ready on {} bus",
            if session { "session" } else { "system" }
        );
        future::race(
            async {
                while let Some(event) = owners.next().await {
                    if let Ok(args) = event.args() {
                        if args.name().as_str().starts_with(':')
                            && args.new_owner().is_none()
                            && tracked_owners.disconnect(args.name().as_str())
                        {
                            // Cleanup must not be dropped if the bounded queue is
                            // full. Waiting here cannot block D-Bus method dispatch.
                            let _ = worker.0.send(Command::Disconnect(args.name().to_string()));
                        }
                    }
                }
            },
            async {
                signals.next().await;
            },
        )
        .await;
        Ok(())
    });
    let _ = worker.0.send(Command::Shutdown);
    thread.join().map_err(|_| "haptic worker panicked")?;
    result
}
