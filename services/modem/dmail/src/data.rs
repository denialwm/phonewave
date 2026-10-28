//! Mobile data on the modem side: the data port and its framing (DPM, WDA),
//! the session (WDS) and the settings it hands out.

use std::net::Ipv4Addr;
use std::time::Duration;

use crate::qmi::{Client, Error, Message};

const DPM_OPEN_PORT: u16 = 0x20;
const WDA_SET_DATA_FORMAT: u16 = 0x20;
const WDS_START_NETWORK: u16 = 0x20;
const WDS_STOP_NETWORK: u16 = 0x21;
pub const WDS_PACKET_SERVICE_STATUS: u16 = 0x22;
const WDS_GET_CURRENT_SETTINGS: u16 = 0x2d;
const WDS_SET_IP_FAMILY: u16 = 0x4d;
const WDS_BIND_MUX_DATA_PORT: u16 = 0xa2;

/// The QMI error for a request that changes nothing.
const NO_EFFECT: u16 = 0x1a;
const LINK_LAYER_RAW_IP: u32 = 2;
const AGGREGATION_QMAP_V5: u32 = 9;
const IPV4: u8 = 4;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Starting a session waits for the network to accept it.
const START_TIMEOUT: Duration = Duration::from_secs(30);

pub const ENDPOINT_EMBEDDED: u32 = 4;

/// The hardware data endpoint between the modem and the SoC's IPA.
#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    pub kind: u32,
    pub iface: u32,
}

impl Endpoint {
    fn bytes(self) -> Vec<u8> {
        let mut v = self.kind.to_le_bytes().to_vec();
        v.extend_from_slice(&self.iface.to_le_bytes());
        v
    }
}

/// Opens the data port on `endpoint`, with the IPA's rx and tx endpoint
/// numbers. Opening an open port succeeds.
pub async fn open_port(dpm: &Client, endpoint: Endpoint, rx: u32, tx: u32) -> Result<(), Error> {
    let mut ports = vec![1];
    ports.extend(endpoint.bytes());
    ports.extend_from_slice(&rx.to_le_bytes());
    ports.extend_from_slice(&tx.to_le_bytes());
    let msg = Message::request(DPM_OPEN_PORT).with(0x11, ports);
    dpm.call(msg, TIMEOUT).await.map(drop)
}

/// Raw IP in QMAPv5 both ways, which the kernel's rmnet expects.
pub async fn set_format(
    wda: &Client,
    endpoint: Endpoint,
    dl_datagrams: u32,
    dl_size: u32,
) -> Result<(), Error> {
    let msg = Message::request(WDA_SET_DATA_FORMAT)
        .with(0x11, LINK_LAYER_RAW_IP.to_le_bytes())
        .with(0x12, AGGREGATION_QMAP_V5.to_le_bytes())
        .with(0x13, AGGREGATION_QMAP_V5.to_le_bytes())
        .with(0x15, dl_datagrams.to_le_bytes())
        .with(0x16, dl_size.to_le_bytes())
        .with(0x17, endpoint.bytes());
    wda.call(msg, TIMEOUT).await.map(drop)
}

/// Ties this WDS client's session to `mux_id` on `endpoint`.
pub async fn bind_mux(wds: &Client, endpoint: Endpoint, mux_id: u8) -> Result<(), Error> {
    let msg = Message::request(WDS_BIND_MUX_DATA_PORT)
        .with(0x10, endpoint.bytes())
        .with(0x11, [mux_id]);
    wds.call(msg, TIMEOUT).await.map(drop)
}

pub async fn prefer_ipv4(wds: &Client) -> Result<(), Error> {
    let msg = Message::request(WDS_SET_IP_FAMILY).with(0x01, [IPV4]);
    wds.call(msg, TIMEOUT).await.map(drop)
}

/// Why the network ended or refused a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndReason {
    pub reason: Option<u16>,
    /// Verbose type and reason.
    pub verbose: Option<(u16, u16)>,
}

impl EndReason {
    /// The session ended because this client stopped it.
    pub fn client_end(&self) -> bool {
        self.verbose == Some((3, 2000))
    }
}

impl std::fmt::Display for EndReason {
    /// The verbose reason in words where dmail knows it, with its numbers;
    /// libqmi's `qmi-enums-wds.h` names them all.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some((kind, reason)) = self.verbose else {
            return match self.reason {
                Some(reason) => write!(f, "reason {reason}"),
                None => f.write_str("no reason given"),
            };
        };
        let words = match (kind, reason) {
            // Internal.
            (2, 202) => "the call ended",
            (2, 206) => "the network ended it",
            (2, 212) => "the modem restarted",
            (2, 214) => "the radio type is not preferred",
            (2, 219) => "the radio went off",
            (2, 227) => "the network's inactivity timer ran out",
            // Call manager.
            (3, 1007) => "packet service not allowed",
            (3, 1008) => "the network cannot derive the phone's identity",
            (3, 1009) => "implicitly detached",
            (3, 1014) => "the radio type changed",
            (3, 1015) => "congestion",
            (3, 1019) => "the radio bearer failed",
            (3, 1022) => "roaming not allowed in this tracking area",
            (3, 1026) => "the radio released the data bearer",
            (3, 1027) => "the network released the signalling connection",
            (3, 1028) => "detached from packet service",
            (3, 1029) => "attaching failed",
            (3, 1031) => "the LTE service request failed",
            (3, 1062) => "radio link failure",
            (3, 1067) => "the network detached the phone and asked it to attach again",
            (3, 1068) => "the network detached the phone",
            (3, 1083) => "detached from packet service",
            (3, 1084) => "no radio available",
            (3, 2000) => "dmail stopped it",
            (3, 2001) => "no service",
            // 3GPP, TS 24.008 and 24.301.
            (6, 8) => "operator-determined barring",
            (6, 26) => "insufficient resources",
            (6, 27) => "unknown or missing APN",
            (6, 29) => "authentication failed",
            (6, 30) => "rejected by the gateway",
            (6, 31) => "activation rejected",
            (6, 33) => "service not subscribed",
            (6, 36) => "regular deactivation",
            (6, 38) => "network failure",
            (6, 39) => "the network asked to attach again",
            _ => "",
        };
        let kind_name = match kind {
            2 => "internal",
            3 => "call manager",
            6 => "3GPP",
            _ => "type",
        };
        match words {
            "" => write!(f, "{kind_name} {reason}"),
            words => write!(f, "{words} ({kind_name} {reason})"),
        }
    }
}

fn end_reason(msg: &Message) -> EndReason {
    EndReason {
        reason: match msg.tlv(0x10) {
            Some(&[a, b]) => Some(u16::from_le_bytes([a, b])),
            _ => None,
        },
        verbose: match msg.tlv(0x11) {
            Some(&[a, b, c, d]) => Some((u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d]))),
            _ => None,
        },
    }
}

/// Starts an IPv4 session on `apn`. Returns the outcome, the session handle on
/// success, and the network's reasons on failure.
pub async fn start(wds: &Client, apn: &str) -> Result<(Result<u32, Error>, EndReason), Error> {
    let msg = Message::request(WDS_START_NETWORK)
        .with(0x14, apn.as_bytes())
        .with(0x19, [IPV4]);
    let reply = wds.request(msg, START_TIMEOUT).await?;
    let outcome = reply.result().and_then(|()| match reply.tlv(0x01) {
        Some(&[a, b, c, d]) => Ok(u32::from_le_bytes([a, b, c, d])),
        _ => Err(Error::Malformed("no session handle")),
    });
    Ok((outcome, end_reason(&reply)))
}

/// Stops the session. One the network already ended counts as stopped.
pub async fn stop(wds: &Client, handle: u32) -> Result<(), Error> {
    let msg = Message::request(WDS_STOP_NETWORK).with(0x01, handle.to_le_bytes());
    match wds.call(msg, TIMEOUT).await {
        Err(Error::Qmi(NO_EFFECT)) => Ok(()),
        other => other.map(drop),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    pub mtu: Option<u32>,
}

/// The modem sends IPv4 addresses as little-endian numbers.
fn ipv4(msg: &Message, t: u8) -> Option<Ipv4Addr> {
    match msg.tlv(t) {
        Some(&[a, b, c, d]) => Some(Ipv4Addr::from(u32::from_le_bytes([a, b, c, d]))),
        _ => None,
    }
}

pub fn parse_settings(msg: &Message) -> Result<Settings, Error> {
    let address = ipv4(msg, 0x1e).ok_or(Error::Malformed("no address"))?;
    let prefix = ipv4(msg, 0x21).map_or(32, |mask| u32::from(mask).leading_ones() as u8);
    Ok(Settings {
        address,
        prefix,
        gateway: ipv4(msg, 0x20),
        dns: [0x15, 0x16].iter().filter_map(|&t| ipv4(msg, t)).collect(),
        mtu: match msg.tlv(0x29) {
            Some(&[a, b, c, d]) => Some(u32::from_le_bytes([a, b, c, d])),
            _ => None,
        },
    })
}

pub async fn settings(wds: &Client) -> Result<Settings, Error> {
    let msg = Message::request(WDS_GET_CURRENT_SETTINGS).with(0x10, 0xffffu32.to_le_bytes());
    parse_settings(&wds.call(msg, TIMEOUT).await?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Disconnected,
    Connected,
    Suspended,
    Authenticating,
    Unknown(u8),
}

/// A packet service status indication: the session's status and, when it
/// ended, why.
pub fn parse_status(msg: &Message) -> Option<(Status, EndReason)> {
    let status = match msg.tlv(0x01)?.first()? {
        1 => Status::Disconnected,
        2 => Status::Connected,
        3 => Status::Suspended,
        4 => Status::Authenticating,
        &other => Status::Unknown(other),
    };
    Some((status, end_reason(msg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings() {
        let msg = Message::request(WDS_GET_CURRENT_SETTINGS)
            .with(0x1e, [0x05, 0x00, 0x40, 0x0a])
            .with(0x21, [0xf8, 0xff, 0xff, 0xff])
            .with(0x15, [0x01, 0x01, 0x01, 0x01])
            .with(0x29, 1500u32.to_le_bytes());
        let s = parse_settings(&msg).unwrap();
        assert_eq!(s.address, Ipv4Addr::new(10, 64, 0, 5));
        assert_eq!(s.prefix, 29);
        assert_eq!(s.dns, [Ipv4Addr::new(1, 1, 1, 1)]);
        assert_eq!((s.gateway, s.mtu), (None, Some(1500)));
    }

    #[test]
    fn parses_a_disconnect() {
        let msg = Message::request(WDS_PACKET_SERVICE_STATUS)
            .with(0x01, [1, 0])
            .with(0x10, 2u16.to_le_bytes())
            .with(0x11, [3, 0, 0x21, 0]);
        let (status, why) = parse_status(&msg).unwrap();
        assert_eq!(status, Status::Disconnected);
        assert_eq!((why.reason, why.verbose), (Some(2), Some((3, 0x21))));
    }
}
