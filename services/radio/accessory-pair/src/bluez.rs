//! The BlueZ calls accessory-pair makes, over its D-Bus API.

use crate::{Address, DeviceView, agent};
use async_io::Timer;
use futures_lite::FutureExt;
use std::{
    collections::HashMap,
    future::Future,
    time::{Duration, Instant},
};
use zbus::{
    Connection, Proxy,
    fdo::{ManagedObjects, ObjectManagerProxy},
    proxy::CacheProperties,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

pub const SERVICE: &str = "org.bluez";
pub const ADAPTER: &str = "org.bluez.Adapter1";
pub const DEVICE: &str = "org.bluez.Device1";
pub const BATTERY: &str = "org.bluez.Battery1";

pub type Result<T> = std::result::Result<T, zbus::Error>;

/// Fails with a timeout error when `f` takes longer than `limit`.
pub async fn within<T>(limit: Duration, f: impl Future<Output = Result<T>>) -> Result<T> {
    f.or(async {
        Timer::after(limit).await;
        Err(zbus::Error::Failure(format!(
            "no answer within {} s",
            limit.as_secs()
        )))
    })
    .await
}

/// The error name of a BlueZ error reply, such as `org.bluez.Error.AlreadyExists`.
pub fn error_name(e: &zbus::Error) -> Option<String> {
    match e {
        zbus::Error::MethodError(name, _, _) => Some(name.to_string()),
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::ZBus(inner) => error_name(inner),
            _ => None,
        },
        _ => None,
    }
}

async fn proxy<'a>(conn: &Connection, path: &'a str, interface: &'a str) -> Result<Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination(SERVICE)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

pub async fn objects(conn: &Connection) -> Result<ManagedObjects> {
    let manager = ObjectManagerProxy::builder(conn)
        .destination(SERVICE)?
        .path("/")?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    Ok(manager.get_managed_objects().await?)
}

fn flag(props: &HashMap<String, OwnedValue>, name: &str) -> bool {
    props
        .get(name)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

fn address_of(props: &HashMap<String, OwnedValue>) -> Option<Address> {
    let v = props.get("Address")?;
    <&str>::try_from(v).ok()?.parse().ok()
}

/// The first powered adapter.
pub fn powered_adapter(objects: &ManagedObjects) -> Option<OwnedObjectPath> {
    let mut paths: Vec<_> = objects
        .iter()
        .filter(|(_, ifaces)| {
            ifaces
                .get(ADAPTER)
                .is_some_and(|props| flag(props, "Powered"))
        })
        .map(|(path, _)| path.clone())
        .collect();
    paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    paths.into_iter().next()
}

/// The device object of `address` under `adapter`, and what BlueZ knows of it.
pub fn find_device(
    objects: &ManagedObjects,
    adapter: &str,
    address: Address,
) -> Option<(OwnedObjectPath, DeviceView)> {
    let prefix = format!("{adapter}/");
    objects.iter().find_map(|(path, ifaces)| {
        let props = ifaces.get(DEVICE)?;
        if !path.as_str().starts_with(&prefix) || address_of(props)? != address {
            return None;
        }
        Some((
            path.clone(),
            DeviceView {
                exists: true,
                paired: flag(props, "Paired") || flag(props, "Bonded"),
                trusted: flag(props, "Trusted"),
                connected: flag(props, "Connected"),
                blocked: flag(props, "Blocked"),
            },
        ))
    })
}

pub fn battery(objects: &ManagedObjects, device: &str) -> Option<u8> {
    let path = objects.keys().find(|p| p.as_str() == device)?;
    let v = objects.get(path)?.get(BATTERY)?.get("Percentage")?;
    u8::try_from(v).ok()
}

/// Scans LE until `address` (or, see `Address::reversed`, its byte-reversed
/// form) shows up, for at most `limit`. Returns the object and the address
/// it was found under.
pub async fn discover(
    conn: &Connection,
    adapter: &str,
    address: Address,
    limit: Duration,
) -> Result<Option<(OwnedObjectPath, Address)>> {
    let a = proxy(conn, adapter, ADAPTER).await?;
    let filter: HashMap<&str, Value> = [("Transport", Value::from("le"))].into();
    a.call::<_, _, ()>("SetDiscoveryFilter", &(filter,)).await?;
    if let Err(e) = a.call::<_, _, ()>("StartDiscovery", &()).await {
        if error_name(&e).as_deref() != Some("org.bluez.Error.InProgress") {
            return Err(e);
        }
    }
    let deadline = Instant::now() + limit;
    let mut found = None;
    while Instant::now() < deadline {
        let objects = objects(conn).await?;
        for candidate in [address, address.reversed()] {
            if let Some((path, _)) = find_device(&objects, adapter, candidate) {
                found = Some((path, candidate));
                break;
            }
        }
        if found.is_some() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
    let _ = a.call::<_, _, ()>("StopDiscovery", &()).await;
    Ok(found)
}

pub async fn register_agent(conn: &Connection) -> Result<()> {
    let m = proxy(conn, "/org/bluez", "org.bluez.AgentManager1").await?;
    let path = zbus::zvariant::ObjectPath::try_from(agent::PATH)?;
    match m
        .call::<_, _, ()>("RegisterAgent", &(path, agent::CAPABILITY))
        .await
    {
        Err(e) if error_name(&e).as_deref() == Some("org.bluez.Error.AlreadyExists") => Ok(()),
        r => r,
    }
}

pub async fn pair(conn: &Connection, device: &str) -> Result<()> {
    let d = proxy(conn, device, DEVICE).await?;
    match within(Duration::from_secs(60), d.call::<_, _, ()>("Pair", &())).await {
        Err(e) if error_name(&e).as_deref() == Some("org.bluez.Error.AlreadyExists") => Ok(()),
        r => r,
    }
}

pub async fn connect(conn: &Connection, device: &str) -> Result<()> {
    let d = proxy(conn, device, DEVICE).await?;
    within(Duration::from_secs(30), d.call::<_, _, ()>("Connect", &())).await
}

pub async fn set_flag(conn: &Connection, device: &str, name: &str, value: bool) -> Result<()> {
    let d = proxy(conn, device, DEVICE).await?;
    d.set_property(name, value).await.map_err(zbus::Error::from)
}
