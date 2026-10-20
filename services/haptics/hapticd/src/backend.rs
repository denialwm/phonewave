use crate::{Config, DeviceConfig, DeviceInfo};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

pub trait Backend: Send {
    fn play(&mut self, duration_ms: u32, intensity: f64) -> io::Result<()>;
    fn stop(&mut self) -> io::Result<()>;
}

pub struct Device {
    pub info: DeviceInfo,
    pub backend: Box<dyn Backend>,
}

pub fn discover(config: &Config) -> io::Result<BTreeMap<String, Device>> {
    let mut devices = BTreeMap::new();
    let mut paths = Vec::new();
    for entry in &config.devices {
        if entry.id.is_empty() || devices.contains_key(&entry.id) {
            return Err(io::Error::other(
                "configured device IDs must be nonempty and unique",
            ));
        }
        let canonical = fs::canonicalize(&entry.path)?;
        if paths.contains(&canonical) {
            return Err(io::Error::other("device configured more than once"));
        }
        paths.push(canonical);
        devices.insert(entry.id.clone(), open(entry)?);
    }
    // Opt-in tagging avoids selecting gamepads, steering wheels or touchpads
    // merely because they expose force feedback.
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("input")?;
    enumerator.match_property("HAPTICD_DEVICE", "1")?;
    for entry in enumerator.scan_devices()? {
        let Some(path) = entry.devnode() else {
            continue;
        };
        if !entry.sysname().as_encoded_bytes().starts_with(b"event") {
            continue;
        }
        if paths.contains(&fs::canonicalize(path)?) {
            continue;
        }
        let id = entry
            .property_value("HAPTICD_ID")
            .and_then(|v| v.to_str())
            .unwrap_or("builtin")
            .to_owned();
        if devices.contains_key(&id) {
            continue;
        }
        match open(&DeviceConfig {
            id: id.clone(),
            backend: "evdev".into(),
            path: path.into(),
            gain: None,
            gain_max: None,
        }) {
            Ok(device) => {
                devices.insert(id, device);
            }
            Err(error) => eprintln!("hapticd: {id}: {error}"),
        }
    }
    Ok(devices)
}

fn open(config: &DeviceConfig) -> io::Result<Device> {
    let (backend, intensity): (Box<dyn Backend>, bool) = match config.backend.as_str() {
        "evdev" => (Box::new(Evdev::open(&config.path)?), true),
        "sysfs" => {
            let backend = Sysfs::open(config)?;
            let intensity = backend.gain.is_some();
            (Box::new(backend), intensity)
        }
        _ => return Err(io::Error::other("backend must be evdev or sysfs")),
    };
    Ok(Device {
        info: DeviceInfo {
            id: config.id.clone(),
            name: config.id.clone(),
            backend: config.backend.clone(),
            intensity,
        },
        backend,
    })
}

struct Sysfs {
    path: PathBuf,
    gain: Option<(String, u32)>,
}

impl Sysfs {
    fn open(config: &DeviceConfig) -> io::Result<Self> {
        for attribute in ["duration", "activate"] {
            OpenOptions::new()
                .write(true)
                .open(config.path.join(attribute))?;
        }
        let gain = match (&config.gain, config.gain_max) {
            (None, None) => None,
            (Some(name), Some(max))
                if max > 0
                    && !name.is_empty()
                    && Path::new(name).components().count() == 1
                    && !name.contains(['/', '.']) =>
            {
                OpenOptions::new()
                    .write(true)
                    .open(config.path.join(name))?;
                Some((name.clone(), max))
            }
            _ => {
                return Err(io::Error::other(
                    "gain requires a simple attribute name and positive gain_max",
                ));
            }
        };
        Ok(Self {
            path: config.path.clone(),
            gain,
        })
    }
}

impl Backend for Sysfs {
    fn play(&mut self, duration_ms: u32, intensity: f64) -> io::Result<()> {
        // Stop before setting gain: some drivers restore their gain on stop.
        self.stop()?;
        if intensity == 0.0 {
            return Ok(());
        }
        if let Some((name, max)) = &self.gain {
            let value = (intensity * f64::from(*max)).round() as u32;
            fs::write(self.path.join(name), format!("{value}\n"))?;
        }
        fs::write(self.path.join("duration"), format!("{duration_ms}\n"))?;
        fs::write(self.path.join("activate"), "1\n")
    }
    fn stop(&mut self) -> io::Result<()> {
        fs::write(self.path.join("activate"), "0\n")
    }
}

const EV_FF: u16 = 0x15;
const FF_RUMBLE: u16 = 0x50;
nix::ioctl_read_buf!(ff_capabilities, b'E', 0x20 + EV_FF as u8, u8);
nix::ioctl_write_ptr!(upload_effect, b'E', 0x80, libc::ff_effect);
nix::ioctl_write_int!(remove_effect, b'E', 0x81);

struct Evdev {
    file: File,
    effect_id: i16,
}

impl Evdev {
    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut bits = [0u8; 16];
        // SAFETY: the ioctl writes only into the supplied capabilities buffer.
        unsafe { ff_capabilities(file.as_raw_fd(), &mut bits) }.map_err(io::Error::from)?;
        if bits[usize::from(FF_RUMBLE) / 8] & (1 << (FF_RUMBLE % 8)) == 0 {
            return Err(io::Error::other("device does not support FF_RUMBLE"));
        }
        Ok(Self {
            file,
            effect_id: -1,
        })
    }
    fn event(&mut self, value: i32) -> io::Result<()> {
        // SAFETY: all-zero input_event is valid, including timeval.
        let mut event: libc::input_event = unsafe { std::mem::zeroed() };
        event.type_ = EV_FF;
        event.code = self.effect_id as u16;
        event.value = value;
        // SAFETY: initialized C structure, read for exactly its size.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&event as *const libc::input_event).cast::<u8>(),
                std::mem::size_of_val(&event),
            )
        };
        self.file.write_all(bytes)
    }
}

impl Backend for Evdev {
    fn play(&mut self, duration_ms: u32, intensity: f64) -> io::Result<()> {
        self.stop()?;
        if intensity == 0.0 {
            return Ok(());
        }
        // SAFETY: zeroed ff_effect is valid; the selected union is filled below.
        let mut effect: libc::ff_effect = unsafe { std::mem::zeroed() };
        effect.type_ = FF_RUMBLE;
        effect.id = self.effect_id;
        effect.replay.length = duration_ms as u16;
        let magnitude = (intensity * f64::from(u16::MAX)).round() as u16;
        // SAFETY: libc represents the C union as an aligned integer array;
        // ff_rumble_effect fits within it and FF_RUMBLE selects this member.
        unsafe {
            effect
                .u
                .as_mut_ptr()
                .cast::<libc::ff_rumble_effect>()
                .write(libc::ff_rumble_effect {
                    strong_magnitude: magnitude,
                    weak_magnitude: magnitude,
                })
        };
        // SAFETY: EVIOCSFF reads/writes one initialized ff_effect.
        unsafe { upload_effect(self.file.as_raw_fd(), std::ptr::from_mut(&mut effect)) }
            .map_err(io::Error::from)?;
        self.effect_id = effect.id;
        self.event(1)
    }
    fn stop(&mut self) -> io::Result<()> {
        if self.effect_id >= 0 {
            self.event(0)?;
        }
        Ok(())
    }
}

impl Drop for Evdev {
    fn drop(&mut self) {
        let _ = self.stop();
        if self.effect_id >= 0 {
            // SAFETY: EVIOCRMFF takes an integer effect ID belonging to this fd.
            let _ =
                unsafe { remove_effect(self.file.as_raw_fd(), self.effect_id as libc::c_ulong) };
        }
    }
}
