//! The SIM (UIM): card status, the provisioning session, PIN1.

use std::time::Duration;

use crate::qmi::{Client, Error, Message};

const VERIFY_PIN: u16 = 0x26;
const REGISTER_EVENTS: u16 = 0x2e;
const GET_CARD_STATUS: u16 = 0x2f;
pub const CARD_STATUS_IND: u16 = 0x32;
const EVENT_CARD_STATUS: u32 = 1 << 0;
const CHANGE_PROVISIONING_SESSION: u16 = 0x38;
/// The GSM/UMTS provisioning sessions: the primary one is subscription 1,
/// the one NAS, WMS and WDS clients use unless bound to another.
pub const SESSION_PRIMARY_GW: u8 = 0;
pub const SESSION_SECONDARY_GW: u8 = 2;
const PIN1: u8 = 1;
/// The QMI error for a request that changes nothing, such as activating an
/// active session.
const NO_EFFECT: u16 = 0x1a;
const TIMEOUT: Duration = Duration::from_secs(10);

pub const CARD_PRESENT: u8 = 1;
pub const APP_USIM: u8 = 2;
pub const APP_READY: u8 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    NotInitialized,
    Required,
    Verified,
    Disabled,
    Blocked,
    PermanentlyBlocked,
    Unknown(u8),
}

impl From<u8> for Pin {
    fn from(state: u8) -> Self {
        match state {
            0 => Pin::NotInitialized,
            1 => Pin::Required,
            2 => Pin::Verified,
            3 => Pin::Disabled,
            4 => Pin::Blocked,
            5 => Pin::PermanentlyBlocked,
            other => Pin::Unknown(other),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub kind: u8,
    pub state: u8,
    pub aid: Vec<u8>,
    pub pin1: Pin,
    pub pin1_retries: u8,
    pub puk1_retries: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub state: u8,
    pub error: u8,
    pub apps: Vec<App>,
}

impl Card {
    pub fn usim(&self) -> Option<&App> {
        self.apps.iter().find(|a| a.kind == APP_USIM)
    }
}

/// The cards, one per slot, and the slots the GSM/UMTS provisioning sessions
/// run on (1-based), if they run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub cards: Vec<Card>,
    pub primary: Option<u8>,
    pub secondary: Option<u8>,
}

/// The Card Status TLV of Get Card Status.
pub fn parse_card_status(v: &[u8]) -> Result<Status, Error> {
    let bad = Error::Malformed("card status");
    let byte = |i: usize| v.get(i).copied().ok_or(Error::Malformed("card status"));
    // A session's app: the slot index in the high byte, the app's in the low
    // one; 0xffff for none.
    let session_slot = |i: usize| match (byte(i), byte(i + 1)) {
        (Ok(0xff), Ok(0xff)) => Ok(None),
        (Ok(_), Ok(slot)) => Ok(Some(slot + 1)),
        (Err(e), _) | (_, Err(e)) => Err(e),
    };
    let (primary, secondary) = (session_slot(0)?, session_slot(4)?);
    let count = byte(8)?;
    let mut pos = 9;
    let mut cards = Vec::new();
    for _ in 0..count {
        let (state, error, app_count) = (byte(pos)?, byte(pos + 4)?, byte(pos + 5)?);
        pos += 6;
        let mut apps = Vec::new();
        for _ in 0..app_count {
            let aid_len = byte(pos + 6)? as usize;
            let app = v
                .get(pos..pos + 14 + aid_len)
                .ok_or(Error::Malformed("card status"))?;
            let pin = 7 + aid_len;
            apps.push(App {
                kind: app[0],
                state: app[1],
                aid: app[7..pin].to_vec(),
                pin1: Pin::from(app[pin + 1]),
                pin1_retries: app[pin + 2],
                puk1_retries: app[pin + 3],
            });
            pos += 14 + aid_len;
        }
        cards.push(Card { state, error, apps });
    }
    if pos > v.len() {
        return Err(bad);
    }
    Ok(Status {
        cards,
        primary,
        secondary,
    })
}

pub async fn card_status(uim: &Client) -> Result<Status, Error> {
    let reply = uim.call(Message::request(GET_CARD_STATUS), TIMEOUT).await?;
    parse_card_status(reply.tlv(0x10).ok_or(Error::Malformed("no card status"))?)
}

/// Asks for a Card Status indication whenever a card, an app or a PIN changes
/// state.
pub async fn report_status(uim: &Client) -> Result<(), Error> {
    let msg = Message::request(REGISTER_EVENTS).with(0x01, EVENT_CARD_STATUS.to_le_bytes());
    uim.call(msg, TIMEOUT).await.map(drop)
}

/// The cards and sessions in a Card Status indication.
pub fn parse_status_indication(msg: &Message) -> Result<Status, Error> {
    parse_card_status(msg.tlv(0x10).ok_or(Error::Malformed("no card status"))?)
}

/// Activates the primary provisioning session on `slot` (1-based) with
/// `aid`. An already active session counts as success.
pub async fn activate(uim: &Client, slot: u8, aid: &[u8]) -> Result<(), Error> {
    let mut info = vec![slot, aid.len() as u8];
    info.extend_from_slice(aid);
    let msg = Message::request(CHANGE_PROVISIONING_SESSION)
        .with(0x01, [SESSION_PRIMARY_GW, 1])
        .with(0x10, info);
    match uim.call(msg, TIMEOUT).await {
        Err(Error::Qmi(NO_EFFECT)) => Ok(()),
        other => other.map(drop),
    }
}

/// Ends a provisioning session, whichever app it runs on. An inactive
/// session counts as success.
pub async fn deactivate(uim: &Client, session: u8) -> Result<(), Error> {
    let msg = Message::request(CHANGE_PROVISIONING_SESSION).with(0x01, [session, 0]);
    match uim.call(msg, TIMEOUT).await {
        Err(Error::Qmi(NO_EFFECT)) => Ok(()),
        other => other.map(drop),
    }
}

/// Verifies PIN1 once. Returns the outcome and the retries left, which the
/// modem reports on failure too.
pub async fn verify_pin1(
    uim: &Client,
    pin: &[u8],
) -> Result<(Result<(), Error>, Option<(u8, u8)>), Error> {
    let mut info = vec![PIN1, pin.len() as u8];
    info.extend_from_slice(pin);
    let msg = Message::request(VERIFY_PIN)
        .with(0x01, [SESSION_PRIMARY_GW, 0])
        .with(0x02, info);
    let reply = uim.request(msg, TIMEOUT).await?;
    let retries = match reply.tlv(0x10) {
        Some(&[verify, unblock]) => Some((verify, unblock)),
        _ => None,
    };
    Ok((reply.result(), retries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_card_with_a_usim() {
        // The primary session on slot 1's first app, no secondary one.
        let mut v = vec![0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1];
        v.extend_from_slice(&[CARD_PRESENT, 0, 3, 10, 0, 1]);
        v.extend_from_slice(&[APP_USIM, 2, 0, 0, 0, 0, 3, 0xa0, 0x00, 0x87]);
        v.extend_from_slice(&[0, 1, 3, 10, 0, 3, 10]);
        let status = parse_card_status(&v).unwrap();
        assert_eq!((status.primary, status.secondary), (Some(1), None));
        assert_eq!(status.cards.len(), 1);
        let usim = status.cards[0].usim().unwrap();
        assert_eq!(usim.aid, [0xa0, 0x00, 0x87]);
        assert_eq!(
            (usim.pin1, usim.pin1_retries, usim.puk1_retries),
            (Pin::Required, 3, 10)
        );
    }

    #[test]
    fn rejects_truncated_status() {
        let v = [
            0,
            1,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
            0xff,
            1,
            CARD_PRESENT,
            0,
            3,
            10,
            0,
            1,
            APP_USIM,
        ];
        assert!(parse_card_status(&v).is_err());
    }

    #[test]
    fn reads_session_slots() {
        let v = [0, 1, 0xff, 0xff, 0, 0, 0xff, 0xff, 0];
        let status = parse_card_status(&v).unwrap();
        assert_eq!((status.primary, status.secondary), (Some(2), Some(1)));
    }
}
