//! The modem as a whole: its operating mode (DMS).

use std::time::Duration;

use crate::qmi::{Client, Error, Message};

const GET_MSISDN: u16 = 0x24;
const GET_IDS: u16 = 0x25;
const GET_OPERATING_MODE: u16 = 0x2d;
const SET_OPERATING_MODE: u16 = 0x2e;
const BIND_SUBSCRIPTION: u16 = 0x54;
pub const ONLINE: u8 = 0;
const LOW_POWER: u8 = 1;
/// The QMI error for data the SIM does not hold.
const NOT_PROVISIONED: u16 = 0x10;
const TIMEOUT: Duration = Duration::from_secs(10);

pub fn mode_name(mode: u8) -> &'static str {
    match mode {
        0 => "online",
        1 => "low power",
        2 => "factory test",
        3 => "offline",
        4 => "resetting",
        5 => "shutting down",
        6 => "persistent low power",
        7 => "mode-only low power",
        _ => "unknown",
    }
}

pub async fn operating_mode(dms: &Client) -> Result<u8, Error> {
    let reply = dms
        .call(Message::request(GET_OPERATING_MODE), TIMEOUT)
        .await?;
    match reply.tlv(0x01) {
        Some(&[mode]) => Ok(mode),
        _ => Err(Error::Malformed("operating mode")),
    }
}

pub async fn set_online(dms: &Client) -> Result<(), Error> {
    set_mode(dms, ONLINE).await
}

/// Radio off: the modem keeps running and the SIM stays unlocked.
pub async fn set_low_power(dms: &Client) -> Result<(), Error> {
    set_mode(dms, LOW_POWER).await
}

async fn set_mode(dms: &Client, mode: u8) -> Result<(), Error> {
    let msg = Message::request(SET_OPERATING_MODE).with(0x01, [mode]);
    dms.call(msg, TIMEOUT).await.map(drop)
}

/// The SIM's own number, if it holds one.
pub async fn msisdn(dms: &Client) -> Result<Option<String>, Error> {
    match dms.call(Message::request(GET_MSISDN), TIMEOUT).await {
        Ok(reply) => Ok(reply
            .tlv(0x01)
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .filter(|n| !n.is_empty())),
        Err(Error::Qmi(NOT_PROVISIONED)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Binds this client to subscription 1 or 2, for the dual-SIM queries.
pub async fn bind_subscription(dms: &Client, subscription: u32) -> Result<(), Error> {
    let msg = Message::request(BIND_SUBSCRIPTION).with(0x01, subscription.to_le_bytes());
    dms.call(msg, TIMEOUT).await.map(drop)
}

/// Whether the modem has a well-formed IMEI. The value itself never leaves
/// this function.
pub async fn imei_valid(dms: &Client) -> Result<bool, Error> {
    let reply = dms.call(Message::request(GET_IDS), TIMEOUT).await?;
    Ok(reply
        .tlv(0x11)
        .is_some_and(|v| luhn_valid(v.strip_suffix(&[0]).unwrap_or(v))))
}

/// 15 digits, not all zero, with a correct Luhn check digit.
fn luhn_valid(digits: &[u8]) -> bool {
    if digits.len() != 15
        || !digits.iter().all(u8::is_ascii_digit)
        || digits.iter().all(|&d| d == b'0')
    {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let n = u32::from(d - b'0') * if i % 2 == 1 { 2 } else { 1 };
            if n > 9 { n - 9 } else { n }
        })
        .sum();
    sum.is_multiple_of(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_imei_digits() {
        // The example IMEI from the GSMA's own documents.
        assert!(luhn_valid(b"490154203237518"));
        assert!(!luhn_valid(b"490154203237519"));
        assert!(!luhn_valid(b"000000000000000"));
        assert!(!luhn_valid(b"49015420323751"));
    }
}
