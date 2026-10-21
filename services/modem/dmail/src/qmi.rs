//! QMI messages as they travel over QRTR: a 7-byte header, then TLVs. There
//! is no QMUX framing; each QRTR socket is one client of one service.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::time::Duration;

use async_io::Timer;
use futures_lite::future;

use crate::qrtr::{Addr, Socket};

/// QMI service ids, as the name service lists them.
pub mod service {
    pub const WDS: u32 = 0x01;
    pub const DMS: u32 = 0x02;
    pub const NAS: u32 = 0x03;
    pub const WMS: u32 = 0x05;
    pub const VOICE: u32 = 0x09;
    pub const UIM: u32 = 0x0b;
    pub const IMS: u32 = 0x12;
    pub const WDA: u32 = 0x1a;
    pub const IMSP: u32 = 0x1f;
    pub const IMSA: u32 = 0x21;
    pub const PDC: u32 = 0x24;
    pub const DPM: u32 = 0x2f;

    pub fn name(id: u32) -> Option<&'static str> {
        Some(match id {
            WDS => "WDS",
            DMS => "DMS",
            NAS => "NAS",
            WMS => "WMS",
            VOICE => "Voice",
            UIM => "UIM",
            IMS => "IMS",
            WDA => "WDA",
            IMSP => "IMSP",
            IMSA => "IMSA",
            PDC => "PDC",
            DPM => "DPM",
            _ => return None,
        })
    }
}

/// The result TLV every response carries.
const TLV_RESULT: u8 = 0x02;

/// QMI errors for information the modem does not have yet, such as the
/// signal with no cell, or the operating mode right after it starts.
pub const INFORMATION_UNAVAILABLE: u16 = 0x4a;
const DEVICE_NOT_READY: u16 = 0x34;
const UIM_UNINITIALIZED: u16 = 0x25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Request,
    Response,
    Indication,
}

impl Kind {
    fn byte(self) -> u8 {
        match self {
            Kind::Request => 0,
            Kind::Response => 2,
            Kind::Indication => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub kind: Kind,
    pub txn: u16,
    pub id: u16,
    tlvs: Vec<(u8, Vec<u8>)>,
}

impl Message {
    pub fn request(id: u16) -> Self {
        Self {
            kind: Kind::Request,
            txn: 0,
            id,
            tlvs: Vec::new(),
        }
    }

    /// Adds a TLV. Builder style: `Message::request(0x20).with(0x01, [1])`.
    pub fn with(mut self, t: u8, value: impl Into<Vec<u8>>) -> Self {
        self.tlvs.push((t, value.into()));
        self
    }

    pub fn tlv(&self, t: u8) -> Option<&[u8]> {
        self.tlvs
            .iter()
            .find(|(k, _)| *k == t)
            .map(|(_, v)| v.as_slice())
    }

    pub fn tlvs(&self) -> impl Iterator<Item = (u8, &[u8])> {
        self.tlvs.iter().map(|(t, v)| (*t, v.as_slice()))
    }

    /// Ok, or the QMI error code the service answered with.
    pub fn result(&self) -> Result<(), Error> {
        match self.tlv(TLV_RESULT) {
            Some(&[r0, r1, e0, e1]) => match u16::from_le_bytes([r0, r1]) {
                0 => Ok(()),
                _ => Err(Error::Qmi(u16::from_le_bytes([e0, e1]))),
            },
            _ => Err(Error::Malformed("missing result TLV")),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let body: usize = self.tlvs.iter().map(|(_, v)| 3 + v.len()).sum();
        let mut out = Vec::with_capacity(7 + body);
        out.push(self.kind.byte());
        out.extend_from_slice(&self.txn.to_le_bytes());
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&(body as u16).to_le_bytes());
        for (t, v) in &self.tlvs {
            out.push(*t);
            out.extend_from_slice(&(v.len() as u16).to_le_bytes());
            out.extend_from_slice(v);
        }
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, Error> {
        if buf.len() < 7 {
            return Err(Error::Malformed("short header"));
        }
        let kind = match buf[0] {
            0 => Kind::Request,
            2 => Kind::Response,
            4 => Kind::Indication,
            _ => return Err(Error::Malformed("unknown message type")),
        };
        let le = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
        let (txn, id, len) = (le(1), le(3), le(5) as usize);
        if len != buf.len() - 7 {
            return Err(Error::Malformed("length mismatch"));
        }
        let mut tlvs = Vec::new();
        let mut rest = &buf[7..];
        while !rest.is_empty() {
            if rest.len() < 3 {
                return Err(Error::Malformed("truncated TLV header"));
            }
            let n = u16::from_le_bytes([rest[1], rest[2]]) as usize;
            if rest.len() < 3 + n {
                return Err(Error::Malformed("truncated TLV"));
            }
            tlvs.push((rest[0], rest[3..3 + n].to_vec()));
            rest = &rest[3 + n..];
        }
        Ok(Self {
            kind,
            txn,
            id,
            tlvs,
        })
    }
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The service answered with this QMI error code.
    Qmi(u16),
    Timeout,
    Malformed(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Qmi(code) => write!(f, "QMI error {code:#06x}"),
            Error::Timeout => f.write_str("no answer from the modem"),
            Error::Malformed(what) => write!(f, "malformed QMI message: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The modem is running but not ready for the request yet: asked again a
    /// moment later, it can succeed.
    pub fn not_ready(&self) -> bool {
        matches!(
            self,
            Error::Qmi(INFORMATION_UNAVAILABLE | DEVICE_NOT_READY | UIM_UNINITIALIZED)
        )
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// One client of one service. Whoever waits on it, a request or
/// `indication`, reads the socket and routes what arrives: responses to their
/// requests, indications to the queue.
pub struct Client {
    sock: Socket,
    service: Addr,
    next_txn: Cell<u16>,
    waiting: RefCell<HashMap<u16, async_channel::Sender<Message>>>,
    queued: async_channel::Sender<Message>,
    indications: async_channel::Receiver<Message>,
}

impl Client {
    pub fn new(service: Addr) -> io::Result<Self> {
        let (queued, indications) = async_channel::unbounded();
        Ok(Self {
            sock: Socket::new()?,
            service,
            next_txn: Cell::new(1),
            waiting: RefCell::default(),
            queued,
            indications,
        })
    }

    /// Reads and routes datagrams until the socket fails.
    async fn pump(&self) -> Error {
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = match self.sock.recv_from(&mut buf).await {
                Ok(got) => got,
                Err(e) => return Error::Io(e),
            };
            if from != self.service {
                continue;
            }
            let Ok(msg) = Message::decode(&buf[..n]) else {
                continue;
            };
            match msg.kind {
                Kind::Response => {
                    if let Some(tx) = self.waiting.borrow_mut().remove(&msg.txn) {
                        let _ = tx.try_send(msg);
                    }
                }
                Kind::Indication => {
                    let _ = self.queued.try_send(msg);
                }
                Kind::Request => {}
            }
        }
    }

    /// The next indication from the service.
    pub async fn indication(&self) -> Result<Message, Error> {
        future::or(
            async { self.indications.recv().await.map_err(|_| Error::Timeout) },
            async { Err(self.pump().await) },
        )
        .await
    }

    /// Sends a request and waits for its response, whatever its result.
    pub async fn request(&self, mut msg: Message, timeout: Duration) -> Result<Message, Error> {
        let txn = self.next_txn.get();
        self.next_txn.set(txn.checked_add(1).unwrap_or(1));
        msg.kind = Kind::Request;
        msg.txn = txn;

        let (tx, rx) = async_channel::bounded(1);
        self.waiting.borrow_mut().insert(txn, tx);
        let _forget = Forget { client: self, txn };
        self.sock.send_to(&msg.encode(), self.service).await?;
        let reply = async { rx.recv().await.map_err(|_| Error::Timeout) };
        let pump = async { Err(self.pump().await) };
        let timer = async {
            Timer::after(timeout).await;
            Err(Error::Timeout)
        };
        future::or(future::or(reply, pump), timer).await
    }

    /// `request`, then the response's result.
    pub async fn call(&self, msg: Message, timeout: Duration) -> Result<Message, Error> {
        let reply = self.request(msg, timeout).await?;
        reply.result()?;
        Ok(reply)
    }
}

/// Drops a request's slot however the wait ends.
struct Forget<'a> {
    client: &'a Client,
    txn: u16,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        self.client.waiting.borrow_mut().remove(&self.txn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut msg = Message::request(0x20).with(0x01, [6, 3, 0]).with(0x13, [0]);
        msg.txn = 7;
        let bytes = msg.encode();
        assert_eq!(
            bytes,
            [0, 7, 0, 0x20, 0, 10, 0, 0x01, 3, 0, 6, 3, 0, 0x13, 1, 0, 0]
        );
        assert_eq!(Message::decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn reads_result() {
        let ok = Message::decode(&[2, 1, 0, 0x22, 0, 7, 0, 2, 4, 0, 0, 0, 0, 0]).unwrap();
        assert!(ok.result().is_ok());
        let err = Message::decode(&[2, 1, 0, 0x22, 0, 7, 0, 2, 4, 0, 1, 0, 0x0c, 0]).unwrap();
        assert!(matches!(err.result(), Err(Error::Qmi(0x0c))));
    }

    #[test]
    fn rejects_bad_lengths() {
        assert!(Message::decode(&[4, 0, 0, 1, 0, 9, 0, 1, 2, 0]).is_err());
        assert!(Message::decode(&[4, 0, 0, 1, 0, 4, 0, 1, 9, 0, 0]).is_err());
    }
}
