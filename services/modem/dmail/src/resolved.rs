//! Cellular DNS through systemd-resolved: the carrier's servers on the rmnet
//! link, used for every lookup only while the link holds the best default
//! route. See DESIGN.md, Data.

use std::net::Ipv4Addr;

use zbus::Connection;

const NAME: &str = "org.freedesktop.resolve1";
const PATH: &str = "/org/freedesktop/resolve1";
const MANAGER: &str = "org.freedesktop.resolve1.Manager";

async fn call<B>(bus: &Connection, method: &str, body: &B) -> zbus::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    bus.call_method(Some(NAME), PATH, Some(MANAGER), method, body)
        .await
        .map(drop)
}

/// Gives link `index` its servers, without LLMNR or mDNS: neither has anyone
/// to ask on a carrier link, and each query would wake the modem.
pub async fn set_link(bus: &Connection, index: u32, servers: &[Ipv4Addr]) -> zbus::Result<()> {
    let index = index as i32;
    let servers: Vec<(i32, Vec<u8>)> = servers
        .iter()
        .map(|s| (libc::AF_INET, s.octets().to_vec()))
        .collect();
    call(bus, "SetLinkDNS", &(index, servers)).await?;
    call(bus, "SetLinkLLMNR", &(index, "no")).await?;
    call(bus, "SetLinkMulticastDNS", &(index, "no")).await
}

/// Whether lookups for any name go to link `index`.
pub async fn set_default_route(bus: &Connection, index: u32, on: bool) -> zbus::Result<()> {
    call(bus, "SetLinkDefaultRoute", &(index as i32, on)).await
}
