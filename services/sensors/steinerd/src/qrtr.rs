//! QRTR: the datagram bus to the DSP services, and the kernel name service
//! that says which service lives at which node and port.

use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use async_io::Async;

const AF_QIPCRTR: libc::c_int = 42;
/// The name service listens on this port of the local node.
const PORT_CTRL: u32 = 0xffff_fffe;
const TYPE_NEW_SERVER: u32 = 4;
const TYPE_DEL_SERVER: u32 = 5;
const TYPE_NEW_LOOKUP: u32 = 10;

/// A node and port on the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Addr {
    pub node: u32,
    pub port: u32,
}

#[repr(C)]
struct SockaddrQrtr {
    family: libc::sa_family_t,
    node: u32,
    port: u32,
}

const SOCKADDR_LEN: libc::socklen_t = mem::size_of::<SockaddrQrtr>() as libc::socklen_t;

impl SockaddrQrtr {
    fn new(addr: Addr) -> Self {
        Self {
            family: AF_QIPCRTR as libc::sa_family_t,
            node: addr.node,
            port: addr.port,
        }
    }

    fn zeroed() -> Self {
        Self::new(Addr { node: 0, port: 0 })
    }

    fn addr(&self) -> Addr {
        Addr {
            node: self.node,
            port: self.port,
        }
    }
}

/// A QRTR datagram socket, bound to a port of its own from the start.
pub struct Socket(Async<OwnedFd>);

impl Socket {
    pub fn new() -> io::Result<Self> {
        let flags = libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK;
        // SAFETY: plain socket(2); the descriptor is owned right below.
        let fd = unsafe { libc::socket(AF_QIPCRTR, flags, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is a fresh descriptor that nothing else owns.
        let sock = Self(Async::new(unsafe { OwnedFd::from_raw_fd(fd) })?);
        // Unbound, it would fail to receive until its first send. Port 0 asks
        // the kernel for a free one.
        let sa = SockaddrQrtr::new(Addr {
            node: sock.local_addr()?.node,
            port: 0,
        });
        // SAFETY: sa is a valid sockaddr_qrtr for the call.
        let r = unsafe { libc::bind(sock.0.as_raw_fd(), (&raw const sa).cast(), SOCKADDR_LEN) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sock)
    }

    /// This socket's address. The node is always the local node.
    pub fn local_addr(&self) -> io::Result<Addr> {
        let mut sa = SockaddrQrtr::zeroed();
        let mut len = SOCKADDR_LEN;
        // SAFETY: sa and len describe a writable sockaddr_qrtr.
        let r = unsafe { libc::getsockname(self.0.as_raw_fd(), (&raw mut sa).cast(), &mut len) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sa.addr())
    }

    pub async fn send_to(&self, buf: &[u8], to: Addr) -> io::Result<()> {
        let sa = SockaddrQrtr::new(to);
        self.0
            .write_with(|fd| {
                // SAFETY: buf and sa stay alive for the call.
                let n = unsafe {
                    libc::sendto(
                        fd.as_raw_fd(),
                        buf.as_ptr().cast(),
                        buf.len(),
                        0,
                        (&raw const sa).cast(),
                        SOCKADDR_LEN,
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            })
            .await
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, Addr)> {
        self.0
            .read_with(|fd| {
                let mut sa = SockaddrQrtr::zeroed();
                let mut len = SOCKADDR_LEN;
                // SAFETY: buf, sa and len are writable for the call.
                let n = unsafe {
                    libc::recvfrom(
                        fd.as_raw_fd(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        0,
                        (&raw mut sa).cast(),
                        &mut len,
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok((n as usize, sa.addr()))
                }
            })
            .await
    }
}

/// A service registered with the name service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    pub id: u32,
    pub version: u8,
    pub instance: u32,
    pub addr: Addr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Up(Service),
    Down(Service),
    /// Every service registered when the lookup started has been reported.
    Listed,
}

/// Follows the name service: first what is registered, then what comes and
/// goes, for as long as it lives.
pub struct Lookup {
    sock: Socket,
}

impl Lookup {
    /// Follows one service id, or every service for 0.
    pub async fn new(service: u32) -> io::Result<Self> {
        let sock = Socket::new()?;
        let ns = Addr {
            node: sock.local_addr()?.node,
            port: PORT_CTRL,
        };
        let mut pkt = [0u8; 20];
        pkt[0..4].copy_from_slice(&TYPE_NEW_LOOKUP.to_le_bytes());
        pkt[4..8].copy_from_slice(&service.to_le_bytes());
        sock.send_to(&pkt, ns).await?;
        Ok(Self { sock })
    }

    pub async fn next(&self) -> io::Result<Event> {
        let mut buf = [0u8; 64];
        loop {
            let (n, from) = self.sock.recv_from(&mut buf).await?;
            if from.port != PORT_CTRL {
                continue;
            }
            if let Some(event) = parse_ctrl(&buf[..n]) {
                return Ok(event);
            }
        }
    }
}

/// The lowest instance of `service` registered right now, if any.
pub async fn find(service: u32) -> io::Result<Option<Service>> {
    let lookup = Lookup::new(service).await?;
    let mut best: Option<Service> = None;
    loop {
        match lookup.next().await? {
            Event::Up(s) if best.is_none_or(|b| s.instance < b.instance) => best = Some(s),
            Event::Listed => return Ok(best),
            _ => {}
        }
    }
}

fn parse_ctrl(pkt: &[u8]) -> Option<Event> {
    if pkt.len() < 20 {
        return None;
    }
    let word = |i: usize| u32::from_le_bytes(pkt[i * 4..i * 4 + 4].try_into().unwrap());
    let (cmd, id, raw_instance, node, port) = (word(0), word(1), word(2), word(3), word(4));
    let service = Service {
        id,
        version: raw_instance as u8,
        instance: raw_instance >> 8,
        addr: Addr { node, port },
    };
    match cmd {
        // The name service ends its first listing with an empty record.
        TYPE_NEW_SERVER if id == 0 && port == 0 => Some(Event::Listed),
        TYPE_NEW_SERVER => Some(Event::Up(service)),
        TYPE_DEL_SERVER => Some(Event::Down(service)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(words: [u32; 5]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn splits_version_from_instance() {
        let event = parse_ctrl(&pkt([TYPE_NEW_SERVER, 3, 0x0104, 0, 37]));
        assert_eq!(
            event,
            Some(Event::Up(Service {
                id: 3,
                version: 4,
                instance: 1,
                addr: Addr { node: 0, port: 37 },
            }))
        );
    }

    #[test]
    fn empty_record_ends_listing() {
        assert_eq!(
            parse_ctrl(&pkt([TYPE_NEW_SERVER, 0, 0, 0, 0])),
            Some(Event::Listed)
        );
        assert!(matches!(
            parse_ctrl(&pkt([TYPE_DEL_SERVER, 5, 1, 0, 9])),
            Some(Event::Down(_))
        ));
        assert_eq!(parse_ctrl(&[0; 8]), None);
    }
}
