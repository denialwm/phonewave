//! steinerctl: what steinerd reports, and what the sensor core offers.

use std::error::Error;
use std::time::{Duration, Instant};

use futures_lite::StreamExt;
use steinerd::bus::{NAME, PATH};
use steinerd::pb::{Reader, Value};
use steinerd::ssc::{Core, attr};
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::zvariant::OwnedValue;

const INTERFACE: &str = "org.steinerd.Sensors1";

const USAGE: &str = "usage: steinerctl status
       steinerctl watch [orientation|light|proximity]...
       steinerctl list [DATA_TYPE]...    sensors the core offers
       steinerctl raw DATA_TYPE [HZ]     10 s of events, from the core";

/// What `list` looks up when given nothing.
const TYPES: &[&str] = &[
    "accel",
    "gyro",
    "mag",
    "proximity",
    "ambient_light",
    "gravity",
    "rotv",
    "game_rv",
    "pressure",
    "hall",
    "significant_motion",
    "step_detect",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result: Result<(), Box<dyn Error>> = async_io::block_on(async {
        match args[..] {
            ["status"] => status().await,
            ["watch"] => watch(&["orientation", "light", "proximity"]).await,
            ["watch", ref sensors @ ..] => watch(sensors).await,
            ["list"] => list(TYPES).await,
            ["list", ref types @ ..] => list(types).await,
            ["raw", data_type] => raw(data_type, None).await,
            ["raw", data_type, hz] => raw(data_type, Some(hz.parse()?)).await,
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    });
    if let Err(e) = result {
        eprintln!("steinerctl: {e}");
        std::process::exit(1);
    }
}

async fn properties() -> Result<(zbus::Connection, PropertiesProxy<'static>), Box<dyn Error>> {
    let conn = zbus::Connection::system().await?;
    let props = PropertiesProxy::builder(&conn)
        .destination(NAME)?
        .path(PATH)?
        .build()
        .await?;
    Ok((conn, props))
}

fn show(name: &str, value: &OwnedValue) -> String {
    let text = match (
        value.downcast_ref::<bool>(),
        value.downcast_ref::<f64>(),
        value.downcast_ref::<&str>(),
    ) {
        (Ok(b), _, _) => b.to_string(),
        (_, Ok(f), _) => format!("{f:.1}"),
        (_, _, Ok(s)) => s.to_owned(),
        _ => format!("{value:?}"),
    };
    format!("{name}={text}")
}

async fn status() -> Result<(), Box<dyn Error>> {
    let (_conn, props) = properties().await?;
    let all = props.get_all(InterfaceName::try_from(INTERFACE)?).await?;
    let mut all: Vec<_> = all.iter().collect();
    all.sort_by_key(|(name, _)| name.as_str());
    for (name, value) in all {
        println!("{}", show(name, value));
    }
    Ok(())
}

/// Claims the sensors and prints every change until interrupted.
async fn watch(sensors: &[&str]) -> Result<(), Box<dyn Error>> {
    let (conn, props) = properties().await?;
    let mut changes = props.receive_properties_changed().await?;
    for sensor in sensors {
        conn.call_method(Some(NAME), PATH, Some(INTERFACE), "Claim", &(sensor,))
            .await?;
    }
    while let Some(signal) = changes.next().await {
        let args = signal.args()?;
        let line: Vec<String> = args
            .changed_properties()
            .iter()
            .filter_map(|(name, v)| Some(show(name, &v.try_to_owned().ok()?)))
            .collect();
        println!("{}", line.join(" "));
    }
    Ok(())
}

async fn list(types: &[&str]) -> Result<(), Box<dyn Error>> {
    let core = Core::connect(Duration::from_secs(3)).await?;
    for data_type in types {
        let uids = match core.suids(data_type).await {
            Ok(uids) => uids,
            Err(e) => {
                println!("{data_type}: {e}");
                continue;
            }
        };
        if uids.is_empty() {
            println!("{data_type}: none");
        }
        for uid in uids {
            match core.attributes(uid).await {
                Ok(a) => println!(
                    "{data_type}: {uid} {} {} {} rates={:?}",
                    a.str(attr::VENDOR).unwrap_or("?").trim(),
                    a.str(attr::NAME).unwrap_or("?").trim(),
                    if a.on_change() {
                        "on-change"
                    } else {
                        "continuous"
                    },
                    a.rates(),
                ),
                Err(e) => println!("{data_type}: {uid} attributes: {e}"),
            }
        }
    }
    Ok(())
}

/// Streams one sensor straight from the core and prints each event's fields.
async fn raw(data_type: &str, hz: Option<f32>) -> Result<(), Box<dyn Error>> {
    let core = Core::connect(Duration::from_secs(3)).await?;
    let uid = *core
        .suids(data_type)
        .await?
        .first()
        .ok_or("no such sensor")?;
    core.enable(uid, hz).await?;
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        for e in core.events().await? {
            let fields: Vec<String> = Reader::new(&e.payload)
                .map(|(f, v)| match v {
                    Value::Varint(n) => format!("{f}={n}"),
                    Value::Fixed32(_) => format!("{f}={:.3}", v.as_f32().unwrap_or_default()),
                    Value::Fixed64(n) => format!("{f}=0x{n:x}"),
                    Value::Bytes(_) => format!("{f}={:?}", v.floats()),
                })
                .collect();
            println!("{} msg={} {}", e.timestamp, e.msg_id, fields.join(" "));
        }
    }
    core.disable(uid).await?;
    Ok(())
}
