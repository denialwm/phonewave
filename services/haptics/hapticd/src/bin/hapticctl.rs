use hapticd::{BUS_NAME, DeviceInfo, INTERFACE, OBJECT_PATH, Step};
use std::{error::Error, time::Duration};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let session = args.first().is_some_and(|s| s == "--session");
    if session {
        args.remove(0);
    }
    if args.is_empty() || args[0] == "--help" {
        println!(
            "hapticctl [--session] list\nhapticctl [--session] vibrate MS INTENSITY [DEVICE]\nhapticctl [--session] pattern MS:INTENSITY,... [DEVICE]\nhapticctl [--session] rescan\nKeeps its D-Bus connection alive until the effect finishes."
        );
        return Ok(());
    }
    let builder = if session {
        zbus::blocking::connection::Builder::session()?
    } else {
        zbus::blocking::connection::Builder::system()?
    };
    let connection = builder.method_timeout(Duration::from_secs(3)).build()?;
    let proxy = zbus::blocking::Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE)?;
    match args[0].as_str() {
        "list" if args.len() == 1 => {
            let devices: Vec<DeviceInfo> = proxy.call("ListDevices", &())?;
            for device in devices {
                println!(
                    "{}\t{}\tintensity={}",
                    device.id, device.backend, device.intensity
                );
            }
        }
        "rescan" if args.len() == 1 => {
            proxy.call::<_, _, ()>("Rescan", &())?;
        }
        "vibrate" if (3..=4).contains(&args.len()) => {
            let duration: u32 = args[1].parse()?;
            let strength: f64 = args[2].parse()?;
            hapticd::validate_pattern(&[(duration, strength)])?;
            let id: u64 = proxy.call(
                "Vibrate",
                &(
                    args.get(3).map(String::as_str).unwrap_or(""),
                    duration,
                    strength,
                ),
            )?;
            println!("effect {id}");
            std::thread::sleep(Duration::from_millis(u64::from(duration) + 20));
        }
        "pattern" if (2..=3).contains(&args.len()) => {
            let steps: Vec<Step> = args[1]
                .split(',')
                .map(|step| {
                    let (ms, strength) = step.split_once(':').ok_or("expected MS:INTENSITY")?;
                    Ok::<_, Box<dyn Error>>((ms.parse()?, strength.parse()?))
                })
                .collect::<Result<_, _>>()?;
            hapticd::validate_pattern(&steps)?;
            let duration: u64 = steps.iter().map(|s| u64::from(s.0)).sum();
            let id: u64 = proxy.call(
                "PlayPattern",
                &(args.get(2).map(String::as_str).unwrap_or(""), steps),
            )?;
            println!("effect {id}");
            std::thread::sleep(Duration::from_millis(duration + 20));
        }
        _ => return Err("invalid arguments; see hapticctl --help".into()),
    }
    Ok(())
}
