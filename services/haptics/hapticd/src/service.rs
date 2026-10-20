use crate::{
    Config, DeviceInfo, Step,
    engine::{Command, Handle},
    validate_pattern,
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use zbus::{Connection, fdo, message::Header, zvariant::OwnedObjectPath};

#[derive(Clone, Default)]
pub struct Owners(Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>);

impl Owners {
    fn track(&self, name: &str) -> Arc<AtomicBool> {
        self.0
            .lock()
            .unwrap()
            .entry(name.into())
            .or_insert_with(|| Arc::new(AtomicBool::new(true)))
            .clone()
    }

    pub fn disconnect(&self, name: &str) -> bool {
        if let Some(alive) = self.0.lock().unwrap().remove(name) {
            alive.store(false, Ordering::Release);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_invalidates_in_flight_requests_only_for_that_caller() {
        let owners = Owners::default();
        let pending = owners.track(":1.10");
        let another_request = owners.track(":1.10");
        let other_caller = owners.track(":1.11");
        assert!(owners.disconnect(":1.10"));
        assert!(!pending.load(Ordering::Acquire));
        assert!(!another_request.load(Ordering::Acquire));
        assert!(other_caller.load(Ordering::Acquire));
        assert!(!owners.disconnect(":1.99"));
    }
}

pub struct Service {
    pub worker: Handle,
    pub config: Config,
    pub session: bool,
    pub owners: Owners,
}

impl Service {
    async fn authorize(
        &self,
        connection: &Connection,
        header: &Header<'_>,
        admin: bool,
    ) -> fdo::Result<(String, Arc<AtomicBool>)> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("missing bus sender".into()))?;
        // Register before credential queries so disconnects during authorization
        // also invalidate requests that have not reached the worker yet.
        let alive = self.owners.track(sender.as_str());
        let bus = fdo::DBusProxy::new(connection).await?;
        let uid = bus.get_connection_unix_user(sender.clone().into()).await?;
        // The session bus is an explicit development mode and allows only the
        // daemon's own UID. System mode derives identity from the bus daemon.
        if (self.session && uid == unsafe { libc::geteuid() }) || uid == 0 {
            return Ok((sender.to_string(), alive));
        }
        if !admin && self.config.allow_uids.contains(&uid) {
            return Ok((sender.to_string(), alive));
        }
        if !admin && !self.session {
            let pid = bus
                .get_connection_unix_process_id(sender.clone().into())
                .await?;
            let manager = zbus::Proxy::new(
                connection,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
            )
            .await?;
            if let Ok(path) = manager
                .call::<_, _, OwnedObjectPath>("GetSessionByPID", &(pid,))
                .await
            {
                let session = zbus::Proxy::new(
                    connection,
                    "org.freedesktop.login1",
                    path,
                    "org.freedesktop.login1.Session",
                )
                .await?;
                let active: bool = session.get_property("Active").await?;
                let remote: bool = session.get_property("Remote").await?;
                if active && !remote {
                    return Ok((sender.to_string(), alive));
                }
            }
        }
        Err(fdo::Error::AccessDenied(
            "requires an active local session, configured UID, or root".into(),
        ))
    }
}

#[zbus::interface(name = "org.hapticd.Haptics1")]
impl Service {
    async fn list_devices(&self) -> fdo::Result<Vec<DeviceInfo>> {
        self.worker
            .request(Command::List)
            .await
            .map_err(fdo::Error::Failed)
    }

    async fn vibrate(
        &self,
        device: String,
        duration_ms: u32,
        intensity: f64,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<u64> {
        self.play_pattern(device, vec![(duration_ms, intensity)], header, connection)
            .await
    }

    async fn play_pattern(
        &self,
        device: String,
        steps: Vec<Step>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<u64> {
        validate_pattern(&steps).map_err(fdo::Error::InvalidArgs)?;
        let (owner, alive) = self.authorize(connection, &header, false).await?;
        self.worker
            .request(|reply| Command::Play {
                owner,
                alive,
                device,
                steps,
                reply,
            })
            .await
            .map_err(fdo::Error::Failed)
    }

    async fn cancel(
        &self,
        effect_id: u64,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        let (owner, _) = self.authorize(connection, &header, false).await?;
        self.worker
            .request(|reply| Command::Cancel {
                owner,
                id: effect_id,
                reply,
            })
            .await
            .map_err(fdo::Error::Failed)
    }

    async fn rescan(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.authorize(connection, &header, true).await?;
        self.worker
            .request(Command::Rescan)
            .await
            .map_err(fdo::Error::Failed)
    }

    #[zbus(property)]
    fn default_device(&self) -> &str {
        &self.config.default_device
    }

    #[zbus(property)]
    fn max_duration_ms(&self) -> u32 {
        crate::MAX_DURATION_MS
    }
}
