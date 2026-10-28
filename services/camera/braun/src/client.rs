//! The client side: the cameras braun offers, and a session with it.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use zbus::zvariant::OwnedValue;

use crate::proto::{self, Reply, Request, Shared};
use crate::{INTERFACE, NAME, PATH};

/// A camera as braun describes it.
#[derive(Clone, Debug)]
pub struct Info {
    pub id: String,
    pub name: String,
    /// front, back or external.
    pub facing: String,
    /// main, ultrawide, telephoto, auxiliary, or empty when the device does
    /// not say.
    pub role: String,
    /// The zoom at which the main camera shows what this one does; none
    /// when the device does not say.
    pub zoom: Option<f32>,
    /// The frame, upright.
    pub width: u32,
    pub height: u32,
    pub lens: bool,
    pub tuned: bool,
}

fn proxy() -> zbus::Result<zbus::blocking::Proxy<'static>> {
    let connection = zbus::blocking::Connection::system()?;
    zbus::blocking::Proxy::new_owned(connection, NAME, PATH, INTERFACE)
}

/// The cameras braun offers.
pub fn cameras() -> zbus::Result<Vec<Info>> {
    let cameras: Vec<HashMap<String, OwnedValue>> = proxy()?.get_property("Cameras")?;
    Ok(cameras
        .into_iter()
        .map(|camera| {
            let text = |key: &str| {
                camera
                    .get(key)
                    .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
                    .unwrap_or_default()
            };
            let number = |key: &str| {
                camera
                    .get(key)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(0)
            };
            let flag = |key: &str| {
                camera
                    .get(key)
                    .and_then(|value| bool::try_from(value).ok())
                    .unwrap_or(false)
            };
            Info {
                id: text("id"),
                name: text("name"),
                facing: text("facing"),
                role: text("role"),
                zoom: camera
                    .get("zoom")
                    .and_then(|value| f64::try_from(value).ok())
                    .map(|zoom| zoom as f32),
                width: number("width"),
                height: number("height"),
                lens: flag("lens"),
                tuned: flag("tuned"),
            }
        })
        .collect())
}

/// A session with braun.
pub struct Session {
    socket: OwnedFd,
    /// The open camera's preview slots, and their size.
    frames: Option<(Shared, usize)>,
}

impl Session {
    pub fn connect() -> zbus::Result<Self> {
        let socket: zbus::zvariant::OwnedFd = proxy()?.call("Connect", &())?;
        Ok(Self {
            socket: socket.into(),
            frames: None,
        })
    }

    /// The socket, to wait on for the next reply.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    pub fn send(&self, request: &Request, fds: &[BorrowedFd<'_>]) -> io::Result<()> {
        proto::send(self.socket.as_fd(), &request.encode(), fds)
    }

    /// The next reply; without `wait`, `None` when there is none yet. On
    /// `Opened`, the camera's preview slots are mapped for `frame`.
    pub fn next(&mut self, wait: bool) -> io::Result<Option<Reply>> {
        loop {
            let Some((bytes, fds)) = proto::recv(self.socket.as_fd(), wait)? else {
                return Ok(None);
            };
            let Some(reply) = Reply::decode(&bytes) else {
                continue;
            };
            if let Reply::Opened {
                slots, slot_bytes, ..
            } = reply
            {
                let fd = fds
                    .into_iter()
                    .next()
                    .ok_or_else(|| io::Error::other("braun opened a camera without its frames"))?;
                let slot_bytes = slot_bytes as usize;
                self.frames = Some((Shared::open(fd, slots as usize * slot_bytes)?, slot_bytes));
            }
            return Ok(Some(reply));
        }
    }

    /// The preview a `Frame` reply names, BGRA. Hand the slot back with
    /// `Request::Release` once it is read.
    pub fn frame(&self, slot: u32, width: u32, height: u32) -> Option<&[u8]> {
        let (shared, slot_bytes) = self.frames.as_ref()?;
        let length = width as usize * height as usize * 4;
        if length > *slot_bytes {
            return None;
        }
        shared.read(slot as usize * slot_bytes, length)
    }
}
