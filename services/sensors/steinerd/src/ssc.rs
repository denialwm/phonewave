//! The Snapdragon Sensor Core: one QMI service on the ADSP that fronts every
//! sensor. Requests and events are protobuf messages addressed by a sensor's
//! 128-bit UID; the QMI layer only carries them.

use std::io;
use std::time::Duration;

use async_io::Timer;
use futures_lite::future;

use crate::pb::{self, Reader, Value, Writer};
use crate::qmi::{self, Client, Message};
use crate::qrtr;

/// The sensor core's QMI service id.
pub const SERVICE: u32 = 400;

const QMI_REQUEST: u16 = 0x20;
const QMI_EVENT_SMALL: u16 = 0x21;
const QMI_EVENT_LARGE: u16 = 0x22;

/// Asks the core to report events in the large indication, which has room
/// for attribute lists.
const REPORT_LARGE: u8 = 1;

/// Message ids, both ways.
pub mod msg {
    pub const GET_ATTRIBUTES: u32 = 1;
    pub const DISABLE: u32 = 10;
    pub const ATTRIBUTES: u32 = 128;
    pub const REQUEST_SUID: u32 = 512;
    pub const ENABLE_CONTINUOUS: u32 = 513;
    pub const ENABLE_ON_CHANGE: u32 = 514;
    /// The SUID lookup's answer, and also the acknowledgement of an enable.
    pub const SUID_OR_CONFIG: u32 = 768;
    pub const PROXIMITY_EVENT: u32 = 769;
    /// A calibration's bias, from a `_cal` sensor.
    pub const CALIBRATION: u32 = 1022;
    pub const MEASUREMENT: u32 = 1025;
}

/// Attribute ids the daemon reads.
pub mod attr {
    pub const NAME: i32 = 0;
    pub const VENDOR: i32 = 1;
    pub const TYPE: i32 = 2;
    pub const AVAILABLE: i32 = 3;
    pub const RATES: i32 = 6;
    pub const STREAM_TYPE: i32 = 16;
    pub const PHYSICAL: i32 = 21;
}

const PROCESSOR_APPS: u64 = 1;
const SUSPEND_WAKEUP: u64 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Uid {
    pub low: u64,
    pub high: u64,
}

impl Uid {
    /// The lookup service that maps a data type to the sensors serving it.
    pub const LOOKUP: Uid = Uid {
        low: 0xabab_abab_abab_abab,
        high: 0xabab_abab_abab_abab,
    };

    fn encode(self) -> Writer {
        Writer::new().fixed64(1, self.low).fixed64(2, self.high)
    }

    fn decode(buf: &[u8]) -> Option<Self> {
        Some(Uid {
            low: pb::field(buf, 1)?.as_u64()?,
            high: pb::field(buf, 2)?.as_u64()?,
        })
    }
}

impl std::fmt::Display for Uid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}{:016x}", self.high, self.low)
    }
}

/// One event from one sensor.
#[derive(Clone, Debug)]
pub struct Event {
    pub uid: Uid,
    pub msg_id: u32,
    /// The DSP clock, 19.2 MHz.
    pub timestamp: u64,
    pub payload: Vec<u8>,
}

/// A value from an attribute list.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    Str(String),
    Float(f32),
    Int(i64),
    Bool(bool),
    List(Vec<AttrValue>),
}

impl AttrValue {
    fn decode(buf: &[u8]) -> Option<Self> {
        let (field, v) = Reader::new(buf).next()?;
        Some(match field {
            1 => AttrValue::List(
                Reader::new(v.as_bytes()?)
                    .filter(|(f, _)| *f == 1)
                    .filter_map(|(_, e)| AttrValue::decode(e.as_bytes()?))
                    .collect(),
            ),
            2 => AttrValue::Str(
                v.as_str()?
                    .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                    .to_owned(),
            ),
            3 => AttrValue::Float(v.as_f32()?),
            4 => AttrValue::Int(v.as_u64()? as i64),
            5 => AttrValue::Bool(v.as_u64()? != 0),
            _ => return None,
        })
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            AttrValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            AttrValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            AttrValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// A sensor's attributes: each id with its values.
#[derive(Clone, Debug, Default)]
pub struct Attributes(pub Vec<(i32, Vec<AttrValue>)>);

impl Attributes {
    pub fn decode(buf: &[u8]) -> Self {
        let mut attrs = Vec::new();
        for (field, v) in Reader::new(buf) {
            let Some(attr) = v.as_bytes().filter(|_| field == 1) else {
                continue;
            };
            let Some(id) = pb::field(attr, 1).and_then(Value::as_u64) else {
                continue;
            };
            let values = pb::field(attr, 2)
                .and_then(Value::as_bytes)
                .map(|array| {
                    Reader::new(array)
                        .filter(|(f, _)| *f == 1)
                        .filter_map(|(_, e)| AttrValue::decode(e.as_bytes()?))
                        .collect()
                })
                .unwrap_or_default();
            attrs.push((id as i32, values));
        }
        Attributes(attrs)
    }

    pub fn get(&self, id: i32) -> &[AttrValue] {
        self.0
            .iter()
            .find(|(i, _)| *i == id)
            .map_or(&[], |(_, v)| v)
    }

    pub fn str(&self, id: i32) -> Option<&str> {
        self.get(id).first()?.as_str()
    }

    pub fn int(&self, id: i32) -> Option<i64> {
        self.get(id).first()?.as_int()
    }

    pub fn bool(&self, id: i32) -> Option<bool> {
        self.get(id).first()?.as_bool()
    }

    /// Whether the sensor reports only when its value changes.
    pub fn on_change(&self) -> bool {
        self.int(attr::STREAM_TYPE) == Some(1)
    }

    /// The sample rates a continuous sensor offers, in Hz.
    pub fn rates(&self) -> Vec<f32> {
        self.get(attr::RATES)
            .iter()
            .filter_map(|v| match v {
                AttrValue::Float(f) => Some(*f),
                _ => None,
            })
            .collect()
    }
}

/// How long the core gets to answer a request or a lookup.
const ANSWER: Duration = Duration::from_secs(2);

/// One client of the sensor core.
pub struct Core {
    client: Client,
    service: qrtr::Service,
    lookup: qrtr::Lookup,
}

impl Core {
    /// Connects to the sensor core, waiting up to `wait` for it to come up.
    pub async fn connect(wait: Duration) -> Result<Self, qmi::Error> {
        let lookup = qrtr::Lookup::new(SERVICE).await?;
        let up = async {
            loop {
                if let qrtr::Event::Up(s) = lookup.next().await? {
                    return Ok::<_, io::Error>(s);
                }
            }
        };
        let timer = async {
            Timer::after(wait).await;
            Err(io::Error::new(io::ErrorKind::NotFound, "no sensor core"))
        };
        let service = future::or(up, timer).await?;
        Ok(Self {
            client: Client::new(service.addr)?,
            service,
            lookup,
        })
    }

    /// Returns once the core leaves the bus, as when the ADSP restarts.
    pub async fn gone(&self) -> io::Result<()> {
        loop {
            if let qrtr::Event::Down(s) = self.lookup.next().await?
                && s.addr == self.service.addr
            {
                return Ok(());
            }
        }
    }

    /// Sends one message to one sensor.
    pub async fn send(
        &self,
        uid: Uid,
        msg_id: u32,
        payload: &[u8],
        passive: bool,
    ) -> Result<(), qmi::Error> {
        let config = Writer::new()
            .varint(1, PROCESSOR_APPS)
            .varint(2, SUSPEND_WAKEUP);
        let mut body = Writer::new().bytes(2, payload);
        if passive {
            body = body.bool(3, true);
        }
        let request = Writer::new()
            .message(1, uid.encode())
            .fixed32(2, msg_id)
            .message(3, config)
            .message(4, body)
            .finish();
        let msg = Message::request(QMI_REQUEST)
            .with(0x01, length_prefixed(&request))
            .with(0x10, [REPORT_LARGE]);
        self.client.call(msg, ANSWER).await?;
        Ok(())
    }

    /// The events of the next indication. An indication the daemon cannot
    /// read yields none.
    pub async fn events(&self) -> Result<Vec<Event>, qmi::Error> {
        loop {
            let ind = self.client.indication().await?;
            if ind.id != QMI_EVENT_SMALL && ind.id != QMI_EVENT_LARGE {
                continue;
            }
            let Some(buf) = ind.tlv(0x02).and_then(unprefix) else {
                continue;
            };
            return Ok(decode_events(buf));
        }
    }

    /// Waits for the first event that `pick` accepts, skipping the rest.
    async fn wait<T>(&self, mut pick: impl FnMut(&Event) -> Option<T>) -> Result<T, qmi::Error> {
        let search = async {
            loop {
                for event in self.events().await? {
                    if let Some(t) = pick(&event) {
                        return Ok(t);
                    }
                }
            }
        };
        let timer = async {
            Timer::after(ANSWER).await;
            Err(qmi::Error::Timeout)
        };
        future::or(search, timer).await
    }

    /// Asks which sensor serves `data_type`; `suid_answer` reads the reply.
    /// With `updates`, the core answers again whenever that changes, as when
    /// a sensor driver comes up late.
    pub async fn request_suids(&self, data_type: &str, updates: bool) -> Result<(), qmi::Error> {
        let request = Writer::new()
            .bytes(1, data_type.as_bytes())
            .bool(2, updates)
            .bool(3, true)
            .finish();
        self.send(Uid::LOOKUP, msg::REQUEST_SUID, &request, false)
            .await
    }

    /// Asks for a sensor's attributes; the reply is an `ATTRIBUTES` event.
    pub async fn request_attributes(&self, uid: Uid) -> Result<(), qmi::Error> {
        let request = Writer::new().bool(2, false).finish();
        self.send(uid, msg::GET_ATTRIBUTES, &request, false).await
    }

    /// The default sensor serving `data_type`, if any.
    pub async fn suids(&self, data_type: &str) -> Result<Vec<Uid>, qmi::Error> {
        self.request_suids(data_type, false).await?;
        self.wait(|e| {
            suid_answer(e)
                .filter(|(t, _)| *t == data_type)
                .map(|(_, uids)| uids)
        })
        .await
    }

    pub async fn attributes(&self, uid: Uid) -> Result<Attributes, qmi::Error> {
        self.request_attributes(uid).await?;
        self.wait(|e| {
            (e.uid == uid && e.msg_id == msg::ATTRIBUTES).then(|| Attributes::decode(&e.payload))
        })
        .await
    }

    /// Starts a sensor's stream: at `rate` Hz for a continuous sensor, on
    /// every change for an on-change one.
    pub async fn enable(&self, uid: Uid, rate: Option<f32>) -> Result<(), qmi::Error> {
        match rate {
            Some(hz) => {
                let config = Writer::new().float(1, hz).finish();
                self.send(uid, msg::ENABLE_CONTINUOUS, &config, false).await
            }
            None => self.send(uid, msg::ENABLE_ON_CHANGE, &[], false).await,
        }
    }

    pub async fn disable(&self, uid: Uid) -> Result<(), qmi::Error> {
        self.send(uid, msg::DISABLE, &[], false).await
    }
}

/// A lookup answer: the data type and the sensors serving it.
pub fn suid_answer(e: &Event) -> Option<(&str, Vec<Uid>)> {
    if e.uid != Uid::LOOKUP || e.msg_id != msg::SUID_OR_CONFIG {
        return None;
    }
    let data_type = pb::field(&e.payload, 1).and_then(Value::as_str)?;
    let uids = Reader::new(&e.payload)
        .filter(|(f, _)| *f == 2)
        .filter_map(|(_, v)| Uid::decode(v.as_bytes()?))
        .collect();
    Some((data_type, uids))
}

/// Sensor-core TLVs carry their protobuf behind a 16-bit length.
fn length_prefixed(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() + 2);
    out.extend_from_slice(&(buf.len() as u16).to_le_bytes());
    out.extend_from_slice(buf);
    out
}

fn unprefix(tlv: &[u8]) -> Option<&[u8]> {
    let n = u16::from_le_bytes(tlv.get(..2)?.try_into().ok()?) as usize;
    tlv.get(2..2 + n)
}

fn decode_events(buf: &[u8]) -> Vec<Event> {
    let Some(uid) = pb::field(buf, 1)
        .and_then(Value::as_bytes)
        .and_then(Uid::decode)
    else {
        return Vec::new();
    };
    Reader::new(buf)
        .filter(|(f, _)| *f == 2)
        .filter_map(|(_, v)| {
            let body = v.as_bytes()?;
            Some(Event {
                uid,
                msg_id: pb::field(body, 1)?.as_u64()? as u32,
                timestamp: pb::field(body, 2).and_then(Value::as_u64).unwrap_or(0),
                payload: pb::field(body, 3)
                    .and_then(Value::as_bytes)
                    .unwrap_or(&[])
                    .to_vec(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_an_indication_with_two_events() {
        let event = |id: u32, payload: &[u8]| {
            Writer::new()
                .fixed32(1, id)
                .fixed64(2, 99)
                .bytes(3, payload)
        };
        let buf = Writer::new()
            .message(1, Uid { low: 1, high: 2 }.encode())
            .message(2, event(msg::MEASUREMENT, b"a"))
            .message(2, event(msg::ATTRIBUTES, b""))
            .finish();
        let tlv = length_prefixed(&buf);
        let events = decode_events(unprefix(&tlv).unwrap());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].uid, Uid { low: 1, high: 2 });
        assert_eq!(events[0].msg_id, msg::MEASUREMENT);
        assert_eq!(events[0].timestamp, 99);
        assert_eq!(events[0].payload, b"a");
        assert!(events[1].payload.is_empty());
    }

    #[test]
    fn decodes_attributes() {
        let value = |v: Writer| Writer::new().message(1, v);
        let attr = |id: u64, values: Writer| Writer::new().varint(1, id).message(2, values);
        let buf = Writer::new()
            .message(1, attr(0, value(Writer::new().bytes(2, b"lsm6dso"))))
            .message(1, attr(16, value(Writer::new().fixed64(4, 1))))
            .message(
                1,
                attr(
                    6,
                    value(Writer::new().float(3, 50.0)).message(1, Writer::new().float(3, 100.0)),
                ),
            )
            .finish();
        let attrs = Attributes::decode(&buf);
        assert_eq!(attrs.str(attr::NAME), Some("lsm6dso"));
        assert!(attrs.on_change());
        assert_eq!(attrs.rates(), [50.0, 100.0]);
    }
}
