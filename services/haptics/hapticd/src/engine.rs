use crate::{
    Config, DeviceInfo, Step,
    backend::{self, Device},
    validate_pattern,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

type Reply<T> = async_channel::Sender<Result<T, String>>;
pub enum Command {
    Play {
        owner: String,
        alive: Arc<AtomicBool>,
        device: String,
        steps: Vec<Step>,
        reply: Reply<u64>,
    },
    Cancel {
        owner: String,
        id: u64,
        reply: Reply<()>,
    },
    Disconnect(String),
    List(Reply<Vec<DeviceInfo>>),
    Rescan(Reply<()>),
    Shutdown,
}

#[derive(Clone)]
pub struct Handle(pub mpsc::SyncSender<Command>);

impl Handle {
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.0
            .try_send(command)
            .map_err(|_| "haptic worker is busy or unavailable".into())
    }
    pub async fn request<T>(&self, make: impl FnOnce(Reply<T>) -> Command) -> Result<T, String> {
        let (tx, rx) = async_channel::bounded(1);
        self.send(make(tx))?;
        rx.recv()
            .await
            .map_err(|_| "haptic worker stopped".to_owned())?
    }
}

struct Effect {
    id: u64,
    owner: String,
    steps: Vec<Step>,
    index: usize,
    deadline: Instant,
}
pub struct Engine {
    devices: BTreeMap<String, Device>,
    effects: BTreeMap<String, Effect>,
    default_device: String,
    next_id: u64,
}

impl Engine {
    pub fn new(devices: BTreeMap<String, Device>, default_device: String) -> Self {
        Self {
            devices,
            default_device,
            effects: BTreeMap::new(),
            next_id: 1,
        }
    }
    fn play(
        &mut self,
        owner: String,
        device: String,
        steps: Vec<Step>,
        now: Instant,
    ) -> Result<u64, String> {
        validate_pattern(&steps)?;
        let selected = if !device.is_empty() {
            device
        } else if !self.default_device.is_empty() {
            self.default_device.clone()
        } else if self.devices.len() == 1 {
            self.devices.keys().next().unwrap().clone()
        } else {
            return Err("no unambiguous default device; choose an ID".into());
        };
        let device = self
            .devices
            .get_mut(&selected)
            .ok_or("haptic device is unavailable")?;
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or("effect IDs exhausted")?;
        // A new effect replaces the previous effect on this actuator. Never
        // queue stale keyboard taps behind an alarm or another application's tap.
        self.effects.remove(&selected);
        if let Err(error) = device.backend.play(steps[0].0, steps[0].1) {
            let _ = device.backend.stop();
            return Err(error.to_string());
        }
        self.effects.insert(
            selected,
            Effect {
                id,
                owner,
                deadline: now + Duration::from_millis(steps[0].0.into()),
                steps,
                index: 0,
            },
        );
        Ok(id)
    }
    fn cancel(&mut self, owner: &str, id: u64) -> Result<(), String> {
        let selected = self.effects.iter().find(|(_, effect)| effect.id == id);
        if let Some((device, effect)) = selected {
            if effect.owner != owner {
                return Err("effect belongs to another D-Bus connection".into());
            }
            let device = device.clone();
            self.effects.remove(&device);
            self.devices
                .get_mut(&device)
                .unwrap()
                .backend
                .stop()
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn disconnect(&mut self, owner: &str) {
        let ids: Vec<_> = self
            .effects
            .values()
            .filter(|e| e.owner == owner)
            .map(|e| e.id)
            .collect();
        for id in ids {
            if let Err(error) = self.cancel(owner, id) {
                eprintln!("hapticd: cancellation: {error}");
            }
        }
    }
    fn tick(&mut self, now: Instant) {
        let due: Vec<_> = self
            .effects
            .iter()
            .filter(|(_, e)| e.deadline <= now)
            .map(|(d, _)| d.clone())
            .collect();
        for id in due {
            let mut effect = self.effects.remove(&id).unwrap();
            let device = self.devices.get_mut(&id).unwrap();
            effect.index += 1;
            // Skip elapsed steps after a scheduler stall rather than replaying
            // stale feedback or stretching the requested pattern indefinitely.
            while effect.index < effect.steps.len() {
                effect.deadline += Duration::from_millis(effect.steps[effect.index].0.into());
                if effect.deadline > now {
                    break;
                }
                effect.index += 1;
            }
            let result = if effect.index == effect.steps.len() {
                device.backend.stop()
            } else {
                let remaining = effect.deadline.duration_since(now).as_millis().max(1) as u32;
                device.backend.play(remaining, effect.steps[effect.index].1)
            };
            if let Err(error) = result {
                let _ = device.backend.stop();
                eprintln!("hapticd: {id}: {error}");
            } else if effect.index < effect.steps.len() {
                self.effects.insert(id, effect);
            }
        }
    }
    fn stop_all(&mut self) {
        for id in self.effects.keys() {
            let _ = self.devices.get_mut(id).unwrap().backend.stop();
        }
        self.effects.clear();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_all();
    }
}

pub fn spawn(config: Config) -> Result<(Handle, thread::JoinHandle<()>), String> {
    let devices = backend::discover(&config).map_err(|e| e.to_string())?;
    eprintln!("hapticd: {} actuator(s) available", devices.len());
    let mut engine = Engine::new(devices, config.default_device.clone());
    let (tx, rx) = mpsc::sync_channel(64);
    let worker = thread::Builder::new()
        .name("haptic-output".into())
        .spawn(move || {
            loop {
                engine.tick(Instant::now());
                let deadline = engine.effects.values().map(|e| e.deadline).min();
                let command = match deadline {
                    Some(deadline) => {
                        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        }
                    }
                    None => match rx.recv() {
                        Ok(command) => command,
                        Err(_) => break,
                    },
                };
                match command {
                    Command::Play {
                        owner,
                        alive,
                        device,
                        steps,
                        reply,
                    } => {
                        let result = if alive.load(Ordering::Acquire) {
                            engine.play(owner, device, steps, Instant::now())
                        } else {
                            Err("caller disconnected before playback".into())
                        };
                        let _ = reply.try_send(result);
                    }
                    Command::Cancel { owner, id, reply } => {
                        let _ = reply.try_send(engine.cancel(&owner, id));
                    }
                    Command::Disconnect(owner) => engine.disconnect(&owner),
                    Command::List(reply) => {
                        let _ = reply.try_send(Ok(engine
                            .devices
                            .values()
                            .map(|d| d.info.clone())
                            .collect()));
                    }
                    Command::Rescan(reply) => {
                        let result = if engine.effects.is_empty() {
                            backend::discover(&config)
                                .map(|devices| engine.devices = devices)
                                .map_err(|e| e.to_string())
                        } else {
                            Err("cannot rescan while effects are active".into())
                        };
                        let _ = reply.try_send(result);
                    }
                    Command::Shutdown => break,
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((Handle(tx), worker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use std::{
        io,
        sync::{Arc, Mutex},
    };
    struct Recorder(Arc<Mutex<Vec<Step>>>);
    impl Backend for Recorder {
        fn play(&mut self, ms: u32, strength: f64) -> io::Result<()> {
            self.0.lock().unwrap().push((ms, strength));
            Ok(())
        }
        fn stop(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().push((0, 0.0));
            Ok(())
        }
    }
    fn fixture() -> (Engine, Arc<Mutex<Vec<Step>>>) {
        let log = Arc::new(Mutex::new(vec![]));
        let device = Device {
            info: DeviceInfo {
                id: "builtin".into(),
                name: "test".into(),
                backend: "test".into(),
                intensity: true,
            },
            backend: Box::new(Recorder(log.clone())),
        };
        (
            Engine::new(BTreeMap::from([("builtin".into(), device)]), String::new()),
            log,
        )
    }
    #[test]
    fn pattern_pauses_finishes_and_does_not_replay_late_steps() {
        let (mut engine, log) = fixture();
        let now = Instant::now();
        engine
            .play(
                ":1.1".into(),
                "".into(),
                vec![(10, 0.5), (20, 0.0), (10, 1.0)],
                now,
            )
            .unwrap();
        engine.tick(now + Duration::from_millis(10));
        engine.tick(now + Duration::from_millis(35));
        engine.tick(now + Duration::from_millis(40));
        assert_eq!(
            *log.lock().unwrap(),
            vec![(10, 0.5), (20, 0.0), (5, 1.0), (0, 0.0)]
        );
        assert!(engine.effects.is_empty());
    }
    #[test]
    fn ownership_replacement_and_disconnect_are_independent() {
        let (mut engine, log) = fixture();
        let now = Instant::now();
        let first = engine
            .play(":1.1".into(), "".into(), vec![(100, 0.5)], now)
            .unwrap();
        assert!(engine.cancel(":1.2", first).is_err());
        let second = engine
            .play(":1.2".into(), "".into(), vec![(20, 1.0)], now)
            .unwrap();
        assert_ne!(first, second);
        engine.disconnect(":1.1");
        assert_eq!(engine.effects.len(), 1);
        engine.disconnect(":1.2");
        assert!(engine.effects.is_empty());
        assert_eq!(log.lock().unwrap().last(), Some(&(0, 0.0)));
    }
    #[test]
    fn invalid_request_does_not_cancel_existing_effect() {
        let (mut engine, _) = fixture();
        let now = Instant::now();
        engine
            .play(":1.1".into(), "".into(), vec![(100, 1.0)], now)
            .unwrap();
        assert!(
            engine
                .play(":1.2".into(), "".into(), vec![(0, 1.0)], now)
                .is_err()
        );
        assert_eq!(engine.effects.len(), 1);
        engine.tick(now + Duration::from_secs(1));
        assert!(engine.effects.is_empty());
    }
}
