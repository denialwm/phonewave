//! Registration, network type, operator and signal (NAS).

use std::time::Duration;

use crate::qmi::{Client, Error, INFORMATION_UNAVAILABLE, Message};

const REGISTER_INDICATIONS: u16 = 0x03;
pub const SERVING_SYSTEM: u16 = 0x24;
const GET_HOME_NETWORK: u16 = 0x25;
const SET_SELECTION_PREFERENCE: u16 = 0x33;
const GET_SELECTION_PREFERENCE: u16 = 0x34;
const SYSTEM_INFO: u16 = 0x4d;
pub const SYSTEM_INFO_IND: u16 = 0x4e;
pub const SIGNAL_INFO: u16 = 0x4f;
const CONFIG_SIGNAL_INFO: u16 = 0x50;
pub const SIGNAL_INFO_IND: u16 = 0x51;
pub const NETWORK_REJECT: u16 = 0x68;
const TIMEOUT: Duration = Duration::from_secs(10);

const RADIO_GSM: u8 = 4;
const RADIO_UMTS: u8 = 5;
const RADIO_LTE: u8 = 8;
const RADIO_TD_SCDMA: u8 = 9;
const RADIO_5GNR: u8 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Technology {
    None,
    G2,
    G3,
    G4,
    G5,
}

impl Technology {
    pub fn as_str(self) -> &'static str {
        match self {
            Technology::None => "none",
            Technology::G2 => "2g",
            Technology::G3 => "3g",
            Technology::G4 => "4g",
            Technology::G5 => "5g",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Serving {
    /// 0 not registered, 1 registered, 2 searching, 3 denied, 4 unknown.
    pub registration: u8,
    /// 0 unknown, 1 attached, 2 detached.
    pub packet: u8,
    pub radios: Vec<u8>,
    pub roaming: Option<bool>,
    /// MCC, MNC and the network's name.
    pub plmn: Option<(u16, u16, String)>,
}

impl Serving {
    pub fn registered(&self) -> bool {
        self.registration == 1
    }

    pub fn packet_attached(&self) -> bool {
        self.packet == 1
    }

    pub fn technology(&self) -> Technology {
        if !self.registered() {
            return Technology::None;
        }
        self.radios
            .iter()
            .map(|&r| match r {
                RADIO_5GNR => Technology::G5,
                RADIO_LTE => Technology::G4,
                RADIO_UMTS | RADIO_TD_SCDMA => Technology::G3,
                RADIO_GSM => Technology::G2,
                _ => Technology::None,
            })
            .max()
            .unwrap_or(Technology::None)
    }
}

/// Serving System, from the response or the indication of the same id.
pub fn parse_serving(msg: &Message) -> Result<Serving, Error> {
    let v = msg.tlv(0x01).ok_or(Error::Malformed("no serving system"))?;
    let n = *v.get(4).ok_or(Error::Malformed("serving system"))? as usize;
    let radios = v
        .get(5..5 + n)
        .ok_or(Error::Malformed("serving system"))?
        .to_vec();
    // Roaming indicator: 0 roaming, 1 home.
    let roaming = match msg.tlv(0x10) {
        Some(&[r]) if r <= 1 => Some(r == 0),
        _ => None,
    };
    let plmn = msg.tlv(0x12).and_then(|p| {
        let mcc = u16::from_le_bytes([*p.first()?, *p.get(1)?]);
        let mnc = u16::from_le_bytes([*p.get(2)?, *p.get(3)?]);
        let len = *p.get(4)? as usize;
        let name = String::from_utf8_lossy(p.get(5..5 + len)?).into_owned();
        Some((mcc, mnc, name))
    });
    Ok(Serving {
        registration: v[0],
        packet: v[2],
        radios,
        roaming,
        plmn,
    })
}

pub async fn serving(nas: &Client) -> Result<Serving, Error> {
    parse_serving(&nas.call(Message::request(SERVING_SYSTEM), TIMEOUT).await?)
}

/// Signal strength, 0 to 100: RSRP on LTE, RSSI on 2G and 3G.
pub fn parse_signal(msg: &Message) -> Option<u8> {
    let percent =
        |value: i32, low: i32, high: i32| ((value - low) * 100 / (high - low)).clamp(0, 100) as u8;
    if let Some(lte) = msg.tlv(0x14).filter(|v| v.len() >= 4) {
        let rsrp = i16::from_le_bytes([lte[2], lte[3]]);
        return Some(percent(rsrp.into(), -140, -98));
    }
    [0x13, 0x12]
        .iter()
        .find_map(|&t| msg.tlv(t).and_then(|v| v.first()))
        .map(|&rssi| percent((rssi as i8).into(), -113, -51))
}

pub async fn signal(nas: &Client) -> Result<Option<u8>, Error> {
    match nas.call(Message::request(SIGNAL_INFO), TIMEOUT).await {
        Ok(reply) => Ok(parse_signal(&reply)),
        Err(Error::Qmi(INFORMATION_UNAVAILABLE)) => Ok(None),
        Err(e) => Err(e),
    }
}

pub const VOICE_CENTRIC: u32 = 1;
pub const DATA_CENTRIC: u32 = 2;
pub const VOICE_CS_PREFERRED: u32 = 2;
pub const VOICE_PS_PREFERRED: u32 = 3;
/// Settings changed with this last until the modem restarts.
const UNTIL_POWER_CYCLE: u8 = 0;

/// Usage (voice or data centric) and voice domain preference.
pub async fn voice_preference(nas: &Client) -> Result<(Option<u32>, Option<u32>), Error> {
    let reply = nas
        .call(Message::request(GET_SELECTION_PREFERENCE), TIMEOUT)
        .await?;
    let word = |t| match reply.tlv(t) {
        Some(&[a, b, c, d]) => Some(u32::from_le_bytes([a, b, c, d])),
        _ => None,
    };
    Ok((word(0x1f), word(0x20)))
}

/// Sets usage and voice domain preference until the modem restarts.
pub async fn set_voice_preference(nas: &Client, usage: u32, voice: u32) -> Result<(), Error> {
    let msg = Message::request(SET_SELECTION_PREFERENCE)
        .with(0x17, [UNTIL_POWER_CYCLE])
        .with(0x21, usage.to_le_bytes())
        .with(0x23, voice.to_le_bytes());
    nas.call(msg, TIMEOUT).await.map(drop)
}

/// A registration the network refused, as 3GPP TS 24.008 and 24.301 number
/// the causes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reject {
    pub radio: u8,
    /// 1 CS, 2 PS, 3 both.
    pub domain: u8,
    pub cause: u8,
}

impl Reject {
    pub fn cause_name(&self) -> &'static str {
        match self.cause {
            2 => "IMSI unknown in HLR",
            3 => "illegal UE",
            5 => "IMEI not accepted",
            6 => "illegal ME",
            7 => "PS services not allowed",
            8 => "PS and non-PS services not allowed",
            9 => "UE identity cannot be derived",
            10 => "implicitly detached",
            11 => "PLMN not allowed",
            12 => "location area not allowed",
            13 => "roaming not allowed in this location area",
            14 => "PS services not allowed in this PLMN",
            15 => "no suitable cells in location area",
            17 => "network failure",
            22 => "congestion",
            _ => "other",
        }
    }
}

pub fn parse_reject(msg: &Message) -> Option<Reject> {
    let byte = |t| match msg.tlv(t) {
        Some(&[b]) => Some(b),
        _ => None,
    };
    Some(Reject {
        radio: byte(0x01)?,
        domain: byte(0x02)?,
        cause: byte(0x03)?,
    })
}

/// Asks for a Network Reject indication on every refusal, keeping the system
/// info indications.
pub async fn report_rejects(nas: &Client) -> Result<(), Error> {
    let msg = Message::request(REGISTER_INDICATIONS).with(0x21, [1, 0]);
    nas.call(msg, TIMEOUT).await.map(drop)
}

/// Asks for indications on registration changes, signal crossing a
/// threshold and network rejects.
pub async fn report_changes(nas: &Client) -> Result<(), Error> {
    let msg = Message::request(REGISTER_INDICATIONS)
        .with(0x13, [1])
        .with(0x19, [1])
        .with(0x21, [1, 0]);
    nas.call(msg, TIMEOUT).await?;
    // Signal indications come only when one of these levels is crossed: RSSI
    // in dBm for 2G and 3G, RSRP in dBm for LTE.
    let mut rssi = vec![4];
    rssi.extend([-105i8, -95, -85, -75].map(|t| t as u8));
    let mut rsrp = vec![4];
    for t in [-130i16, -119, -108, -98] {
        rsrp.extend(t.to_le_bytes());
    }
    let msg = Message::request(CONFIG_SIGNAL_INFO)
        .with(0x10, rssi)
        .with(0x16, rsrp);
    nas.call(msg, TIMEOUT).await.map(drop)
}

/// The SIM's home network, MCC and MNC.
pub async fn home_network(nas: &Client) -> Result<Option<(u16, u16)>, Error> {
    let reply = nas
        .call(Message::request(GET_HOME_NETWORK), TIMEOUT)
        .await?;
    Ok(reply.tlv(0x01).and_then(|v| {
        let mcc = u16::from_le_bytes([*v.first()?, *v.get(1)?]);
        let mnc = u16::from_le_bytes([*v.get(2)?, *v.get(3)?]);
        Some((mcc, mnc))
    }))
}

/// The SIM's reject state: 0 unavailable, 1 available, 2 CS invalid, 3 PS
/// invalid, 4 both invalid. The modem stops trying an invalid domain until the
/// radio goes off and on.
pub async fn sim_reject_state(nas: &Client) -> Result<Option<u32>, Error> {
    let reply = nas.call(Message::request(SYSTEM_INFO), TIMEOUT).await?;
    Ok(match reply.tlv(0x27) {
        Some(&[a, b, c, d]) => Some(u32::from_le_bytes([a, b, c, d])),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indication(tlvs: &[(u8, &[u8])]) -> Message {
        tlvs.iter()
            .fold(Message::request(SERVING_SYSTEM), |m, &(t, v)| m.with(t, v))
    }

    #[test]
    fn reads_lte_registration() {
        let mut plmn = vec![222, 0, 50, 0, 5];
        plmn.extend_from_slice(b"iliad");
        let msg = indication(&[
            (0x01, &[1, 1, 1, 1, 1, RADIO_LTE]),
            (0x10, &[1]),
            (0x12, &plmn),
        ]);
        let s = parse_serving(&msg).unwrap();
        assert!(s.registered() && s.packet_attached());
        assert_eq!(s.technology(), Technology::G4);
        assert_eq!(s.roaming, Some(false));
        assert_eq!(s.plmn, Some((222, 50, "iliad".into())));
    }

    #[test]
    fn unregistered_has_no_technology() {
        let s = parse_serving(&indication(&[(0x01, &[2, 0, 0, 0, 1, RADIO_GSM])])).unwrap();
        assert_eq!(s.technology(), Technology::None);
    }

    #[test]
    fn signal_prefers_lte_rsrp() {
        // RSRP -119 dBm, halfway between -140 and -98.
        let lte = Message::request(SIGNAL_INFO).with(0x14, [0xc0, 0xf6, 0x89, 0xff, 0, 0]);
        assert_eq!(parse_signal(&lte), Some(50));
        let gsm = Message::request(SIGNAL_INFO).with(0x12, [(-82i8) as u8]);
        assert_eq!(parse_signal(&gsm), Some(50));
        assert_eq!(parse_signal(&Message::request(SIGNAL_INFO)), None);
    }

    #[test]
    fn reads_a_reject() {
        let msg = Message::request(NETWORK_REJECT)
            .with(0x01, [RADIO_LTE])
            .with(0x02, [2])
            .with(0x03, [7]);
        let r = parse_reject(&msg).unwrap();
        assert_eq!((r.domain, r.cause_name()), (2, "PS services not allowed"));
    }
}
