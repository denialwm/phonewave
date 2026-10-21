//! The modem's SMS service (WMS): new-message events, acks, raw send, raw
//! read, routes.

use std::time::Duration;

use crate::qmi::{Client, Error, Message};

pub const EVENT_REPORT: u16 = 0x01;
const RAW_SEND: u16 = 0x20;
const RAW_READ: u16 = 0x22;
const DELETE: u16 = 0x24;
const LIST_MESSAGES: u16 = 0x31;
const GET_ROUTES: u16 = 0x33;
const SEND_ACK: u16 = 0x37;

/// Message format for GSM/UMTS point-to-point SMS.
const FORMAT_GW_PP: u8 = 6;
/// Message mode and ack protocol for GSM/UMTS.
const MODE_GW: u8 = 1;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Sending waits for the network, which can take a while on a weak signal.
const SEND_TIMEOUT: Duration = Duration::from_secs(90);

/// A new message, as the event report announces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// Handed straight to us. If `ack` is set, the network waits for
    /// `ack(transaction)` and resends otherwise.
    Transferred {
        ack: bool,
        transaction: u32,
        format: u8,
        tpdu: Vec<u8>,
        on_ims: bool,
    },
    /// Stored by the modem, to be read with `raw_read`.
    Stored { storage: u8, index: u32 },
}

/// The new messages in an event report indication.
pub fn parse_event(msg: &Message) -> Vec<Incoming> {
    let mut out = Vec::new();
    let on_ims = matches!(msg.tlv(0x16), Some(&[1]));
    if let Some(v) = msg.tlv(0x11).filter(|v| v.len() >= 8) {
        let len = u16::from_le_bytes([v[6], v[7]]) as usize;
        if let Some(tpdu) = v.get(8..8 + len) {
            out.push(Incoming::Transferred {
                // Ack indicator: 0 means the network wants an ack.
                ack: v[0] == 0,
                transaction: u32::from_le_bytes([v[1], v[2], v[3], v[4]]),
                format: v[5],
                tpdu: tpdu.to_vec(),
                on_ims,
            });
        }
    }
    if let Some(&[storage, i0, i1, i2, i3]) = msg.tlv(0x10) {
        let index = u32::from_le_bytes([i0, i1, i2, i3]);
        out.push(Incoming::Stored { storage, index });
    }
    out
}

/// Asks for an event report on every new message.
pub async fn report_new_messages(wms: &Client) -> Result<(), Error> {
    let msg = Message::request(EVENT_REPORT).with(0x10, [1]);
    wms.call(msg, TIMEOUT).await.map(drop)
}

pub async fn ack(wms: &Client, transaction: u32, on_ims: bool) -> Result<(), Error> {
    let mut info = transaction.to_le_bytes().to_vec();
    info.extend_from_slice(&[MODE_GW, 1]);
    let msg = Message::request(SEND_ACK)
        .with(0x01, info)
        .with(0x12, [on_ims as u8]);
    wms.call(msg, TIMEOUT).await.map(drop)
}

/// Sends one SMS-SUBMIT TPDU with its SMSC field in front. Returns the
/// outcome, the message reference on success, and the RP and TP causes the
/// modem reports on failure.
pub async fn raw_send(
    wms: &Client,
    pdu: &[u8],
) -> Result<(Result<u16, Error>, Option<(u16, u8)>), Error> {
    let mut data = vec![FORMAT_GW_PP];
    data.extend_from_slice(&(pdu.len() as u16).to_le_bytes());
    data.extend_from_slice(pdu);
    let msg = Message::request(RAW_SEND).with(0x01, data).with(0x13, [0]);
    let reply = wms.request(msg, SEND_TIMEOUT).await?;
    let cause = match reply.tlv(0x12) {
        Some(&[r0, r1, tp]) => Some((u16::from_le_bytes([r0, r1]), tp)),
        _ => None,
    };
    let outcome = reply.result().and_then(|()| match reply.tlv(0x01) {
        Some(&[r0, r1]) => Ok(u16::from_le_bytes([r0, r1])),
        _ => Err(Error::Malformed("no message reference")),
    });
    Ok((outcome, cause))
}

/// Reads a stored message: its format and raw data.
pub async fn raw_read(wms: &Client, storage: u8, index: u32) -> Result<(u8, Vec<u8>), Error> {
    let mut id = vec![storage];
    id.extend_from_slice(&index.to_le_bytes());
    let msg = Message::request(RAW_READ)
        .with(0x01, id)
        .with(0x10, [MODE_GW]);
    let reply = wms.call(msg, TIMEOUT).await?;
    let v = reply
        .tlv(0x01)
        .filter(|v| v.len() >= 4)
        .ok_or(Error::Malformed("raw read"))?;
    let len = u16::from_le_bytes([v[2], v[3]]) as usize;
    let data = v.get(4..4 + len).ok_or(Error::Malformed("raw read"))?;
    Ok((v[1], data.to_vec()))
}

/// Indexes of the messages stored in `storage` (0 SIM, 1 modem).
pub async fn list(wms: &Client, storage: u8) -> Result<Vec<u32>, Error> {
    let msg = Message::request(LIST_MESSAGES)
        .with(0x01, [storage])
        .with(0x11, [MODE_GW]);
    let reply = wms.call(msg, TIMEOUT).await?;
    let v = reply
        .tlv(0x01)
        .filter(|v| v.len() >= 4)
        .ok_or(Error::Malformed("message list"))?;
    let count = u32::from_le_bytes([v[0], v[1], v[2], v[3]]) as usize;
    let entries = v
        .get(4..4 + count * 5)
        .ok_or(Error::Malformed("message list"))?;
    Ok(entries
        .chunks_exact(5)
        .map(|e| u32::from_le_bytes([e[0], e[1], e[2], e[3]]))
        .collect())
}

pub async fn delete(wms: &Client, storage: u8, index: u32) -> Result<(), Error> {
    let msg = Message::request(DELETE)
        .with(0x01, [storage])
        .with(0x10, index.to_le_bytes())
        .with(0x12, [MODE_GW]);
    wms.call(msg, TIMEOUT).await.map(drop)
}

/// Where the modem sends one class of incoming message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    /// 0 point to point.
    pub kind: u8,
    /// 0 to 3 the SMS classes, 4 none.
    pub class: u8,
    /// 0 SIM, 1 modem, 0xff none.
    pub storage: u8,
    /// 0 discard, 1 store and notify, 2 transfer only, 3 transfer and ack.
    pub action: u8,
}

pub fn parse_routes(msg: &Message) -> Result<Vec<Route>, Error> {
    let v = msg
        .tlv(0x01)
        .filter(|v| v.len() >= 2)
        .ok_or(Error::Malformed("routes"))?;
    let count = u16::from_le_bytes([v[0], v[1]]) as usize;
    let entries = v.get(2..2 + count * 4).ok_or(Error::Malformed("routes"))?;
    Ok(entries
        .chunks_exact(4)
        .map(|e| Route {
            kind: e[0],
            class: e[1],
            storage: e[2],
            action: e[3],
        })
        .collect())
}

/// The routes, and whether status reports come to clients like other
/// transferred messages.
pub async fn routes(wms: &Client) -> Result<(Vec<Route>, bool), Error> {
    let reply = wms.call(Message::request(GET_ROUTES), TIMEOUT).await?;
    Ok((parse_routes(&reply)?, reply.tlv(0x10) == Some(&[1])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_transferred_message() {
        let mut v = vec![0, 0x78, 0x56, 0x34, 0x12, FORMAT_GW_PP, 3, 0];
        v.extend_from_slice(&[0x04, 0x0b, 0x91]);
        let msg = Message::request(EVENT_REPORT).with(0x11, v).with(0x16, [0]);
        assert_eq!(
            parse_event(&msg),
            [Incoming::Transferred {
                ack: true,
                transaction: 0x1234_5678,
                format: FORMAT_GW_PP,
                tpdu: vec![0x04, 0x0b, 0x91],
                on_ims: false,
            }]
        );
    }

    #[test]
    fn parses_a_stored_message() {
        let msg = Message::request(EVENT_REPORT).with(0x10, [1, 7, 0, 0, 0]);
        assert_eq!(
            parse_event(&msg),
            [Incoming::Stored {
                storage: 1,
                index: 7
            }]
        );
        assert!(parse_event(&Message::request(EVENT_REPORT)).is_empty());
    }

    #[test]
    fn parses_routes() {
        let msg = Message::request(GET_ROUTES).with(0x01, [2, 0, 0, 0, 1, 1, 0, 1, 0xff, 3]);
        let routes = parse_routes(&msg).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(
            (routes[1].class, routes[1].storage, routes[1].action),
            (1, 0xff, 3)
        );
    }
}
