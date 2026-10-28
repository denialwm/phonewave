//! The few rtnetlink requests dmail needs for its rmnet link: create and
//! delete it, bring it up, give it an address and a default route; and the
//! IPv4 default routes, to know whether the link's is the best.
//!
//! Requests are blocking: the kernel answers them at once.

use std::cell::Cell;
use std::ffi::CString;
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use async_io::Async;

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_NEWADDR: u16 = 20;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;

const NLM_F_REQUEST: u16 = 0x001;
const NLM_F_ACK: u16 = 0x004;
const NLM_F_DUMP: u16 = 0x300;
const NLM_F_REPLACE: u16 = 0x100;
const NLM_F_EXCL: u16 = 0x200;
const NLM_F_CREATE: u16 = 0x400;

const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_LINK: u16 = 5;
const IFLA_LINKINFO: u16 = 18;
const IFLA_INFO_KIND: u16 = 1;
const IFLA_INFO_DATA: u16 = 2;
const IFLA_RMNET_MUX_ID: u16 = 1;
const IFLA_RMNET_FLAGS: u16 = 2;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const RTA_OIF: u16 = 4;
const RTA_PRIORITY: u16 = 6;
const RTA_TABLE: u16 = 15;
/// The multicast group for IPv4 route changes.
const RTMGRP_IPV4_ROUTE: u32 = 0x40;

const IFF_UP: u32 = 1;
const RT_TABLE_MAIN: u8 = 254;
const RTPROT_STATIC: u8 = 4;
const RT_SCOPE_LINK: u8 = 253;
const RTN_UNICAST: u8 = 1;

pub const RMNET_INGRESS_DEAGGREGATION: u32 = 1 << 0;
pub const RMNET_INGRESS_MAP_CKSUMV5: u32 = 1 << 4;
pub const RMNET_EGRESS_MAP_CKSUMV5: u32 = 1 << 5;

pub struct Netlink {
    fd: OwnedFd,
    seq: Cell<u32>,
}

/// Appends one attribute, padded to 4 bytes.
fn attr(buf: &mut Vec<u8>, kind: u16, data: &[u8]) {
    buf.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    buf.extend_from_slice(&kind.to_ne_bytes());
    buf.extend_from_slice(data);
    buf.resize(buf.len().next_multiple_of(4), 0);
}

/// struct ifinfomsg.
fn ifinfo(index: u32, flags: u32, change: u32) -> Vec<u8> {
    let mut m = vec![libc::AF_UNSPEC as u8, 0, 0, 0];
    m.extend_from_slice(&index.to_ne_bytes());
    m.extend_from_slice(&flags.to_ne_bytes());
    m.extend_from_slice(&change.to_ne_bytes());
    m
}

pub fn index(name: &str) -> io::Result<u32> {
    let c = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: c is a valid C string.
    match unsafe { libc::if_nametoindex(c.as_ptr()) } {
        0 => Err(io::Error::last_os_error()),
        i => Ok(i),
    }
}

impl Netlink {
    pub fn new() -> io::Result<Self> {
        // SAFETY: plain socket call; the fd is owned from here on.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            // SAFETY: fd was just opened and nothing else owns it.
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            seq: Cell::new(1),
        })
    }

    /// Sends one message; returns its sequence number.
    fn send(&self, kind: u16, flags: u16, body: &[u8]) -> io::Result<u32> {
        let seq = self.seq.get();
        self.seq.set(seq.wrapping_add(1));
        let mut msg = Vec::with_capacity(16 + body.len());
        msg.extend_from_slice(&((16 + body.len()) as u32).to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        msg.extend_from_slice(&(flags | NLM_F_REQUEST).to_ne_bytes());
        msg.extend_from_slice(&seq.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(body);
        // SAFETY: msg is a valid buffer of its length; the kernel is the
        // default destination of an unbound netlink socket.
        if unsafe { libc::send(self.fd.as_raw_fd(), msg.as_ptr().cast(), msg.len(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(seq)
    }

    /// Reads the kernel's answers to `seq`, message by message, until `each`
    /// says it has what it waited for.
    fn answers(
        &self,
        seq: u32,
        mut each: impl FnMut(u16, &[u8]) -> Option<io::Result<()>>,
    ) -> io::Result<()> {
        let mut buf = vec![0u8; 32768];
        loop {
            // SAFETY: buf is writable for its length.
            let n =
                unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut rest = &buf[..n as usize];
            while rest.len() >= 16 {
                let len = u32::from_ne_bytes(rest[0..4].try_into().unwrap()) as usize;
                let kind = u16::from_ne_bytes(rest[4..6].try_into().unwrap());
                let got = u32::from_ne_bytes(rest[8..12].try_into().unwrap());
                let body = rest.get(16..len.max(16)).unwrap_or_default();
                if got == seq
                    && let Some(done) = each(kind, body)
                {
                    return done;
                }
                rest = rest
                    .get(len.max(16).next_multiple_of(4)..)
                    .unwrap_or_default();
            }
        }
    }

    /// Sends one request and waits for the kernel's ack.
    fn request(&self, kind: u16, flags: u16, body: &[u8]) -> io::Result<()> {
        let seq = self.send(kind, flags | NLM_F_ACK, body)?;
        self.answers(seq, |kind, body| {
            (kind == NLMSG_ERROR && body.len() >= 4).then(|| {
                match i32::from_ne_bytes(body[0..4].try_into().unwrap()) {
                    0 => Ok(()),
                    e => Err(io::Error::from_raw_os_error(-e)),
                }
            })
        })
    }

    /// The main table's IPv4 default routes: link index and metric.
    pub fn default_routes(&self) -> io::Result<Vec<(u32, u32)>> {
        // struct rtmsg, all zero but the family: every IPv4 route.
        let mut rtmsg = [0u8; 12];
        rtmsg[0] = libc::AF_INET as u8;
        let seq = self.send(RTM_GETROUTE, NLM_F_DUMP, &rtmsg)?;
        let mut routes = Vec::new();
        self.answers(seq, |kind, body| match kind {
            NLMSG_DONE => Some(Ok(())),
            NLMSG_ERROR => Some(match body.get(0..4) {
                Some(e) => match i32::from_ne_bytes(e.try_into().unwrap()) {
                    0 => Ok(()),
                    e => Err(io::Error::from_raw_os_error(-e)),
                },
                None => Ok(()),
            }),
            RTM_NEWROUTE => {
                routes.extend(parse_default_route(body));
                None
            }
            _ => None,
        })?;
        Ok(routes)
    }

    /// Creates an rmnet link for `mux_id` on top of `parent`.
    pub fn add_rmnet(&self, parent: &str, name: &str, mux_id: u16, flags: u32) -> io::Result<()> {
        let mut data = Vec::new();
        attr(&mut data, IFLA_RMNET_MUX_ID, &mux_id.to_ne_bytes());
        let mut both = flags.to_ne_bytes().to_vec();
        both.extend_from_slice(&flags.to_ne_bytes());
        attr(&mut data, IFLA_RMNET_FLAGS, &both);
        let mut info = Vec::new();
        attr(&mut info, IFLA_INFO_KIND, b"rmnet");
        attr(&mut info, IFLA_INFO_DATA, &data);

        let mut body = ifinfo(0, 0, 0);
        attr(
            &mut body,
            IFLA_IFNAME,
            CString::new(name)?.as_bytes_with_nul(),
        );
        attr(&mut body, IFLA_LINK, &index(parent)?.to_ne_bytes());
        attr(&mut body, IFLA_LINKINFO, &info);
        self.request(RTM_NEWLINK, NLM_F_CREATE | NLM_F_EXCL, &body)
    }

    pub fn delete_link(&self, name: &str) -> io::Result<()> {
        self.request(RTM_DELLINK, 0, &ifinfo(index(name)?, 0, 0))
    }

    /// Brings a link up, with a new MTU if given.
    pub fn set_up(&self, name: &str, mtu: Option<u32>) -> io::Result<()> {
        let mut body = ifinfo(index(name)?, IFF_UP, IFF_UP);
        if let Some(mtu) = mtu {
            attr(&mut body, IFLA_MTU, &mtu.to_ne_bytes());
        }
        self.request(RTM_NEWLINK, 0, &body)
    }

    pub fn add_address(&self, name: &str, address: Ipv4Addr, prefix: u8) -> io::Result<()> {
        let mut body = vec![libc::AF_INET as u8, prefix, 0, 0];
        body.extend_from_slice(&index(name)?.to_ne_bytes());
        attr(&mut body, IFA_LOCAL, &address.octets());
        attr(&mut body, IFA_ADDRESS, &address.octets());
        self.request(RTM_NEWADDR, NLM_F_CREATE | NLM_F_REPLACE, &body)
    }

    /// A default route straight out of the link: raw IP has no next hop to
    /// resolve.
    pub fn add_default_route(&self, name: &str, metric: u32) -> io::Result<()> {
        let mut body = vec![
            libc::AF_INET as u8,
            0,
            0,
            0,
            RT_TABLE_MAIN,
            RTPROT_STATIC,
            RT_SCOPE_LINK,
            RTN_UNICAST,
        ];
        body.extend_from_slice(&0u32.to_ne_bytes());
        attr(&mut body, RTA_OIF, &index(name)?.to_ne_bytes());
        attr(&mut body, RTA_PRIORITY, &metric.to_ne_bytes());
        self.request(RTM_NEWROUTE, NLM_F_CREATE | NLM_F_REPLACE, &body)
    }
}

/// One RTM_NEWROUTE body, if it is a main-table IPv4 default route: its
/// link index and metric.
fn parse_default_route(body: &[u8]) -> Option<(u32, u32)> {
    let rtmsg = body.get(..12)?;
    let (dst_len, mut table, kind) = (rtmsg[1], u32::from(rtmsg[4]), rtmsg[7]);
    if rtmsg[0] != libc::AF_INET as u8 || dst_len != 0 || kind != RTN_UNICAST {
        return None;
    }
    let (mut oif, mut metric) = (None, 0);
    let mut rest = &body[12..];
    while rest.len() >= 4 {
        let len = u16::from_ne_bytes([rest[0], rest[1]]) as usize;
        let kind = u16::from_ne_bytes([rest[2], rest[3]]);
        let word = rest
            .get(4..8)
            .filter(|_| len == 8)
            .map(|v| u32::from_ne_bytes(v.try_into().unwrap()));
        match kind {
            RTA_OIF => oif = word,
            RTA_PRIORITY => metric = word.unwrap_or(0),
            RTA_TABLE => table = word.unwrap_or(table),
            _ => {}
        }
        rest = rest
            .get(len.max(4).next_multiple_of(4)..)
            .unwrap_or_default();
    }
    (table == u32::from(RT_TABLE_MAIN)).then_some(())?;
    Some((oif?, metric))
}

/// Wakes on IPv4 route changes.
pub struct RouteWatch(Async<OwnedFd>);

impl RouteWatch {
    pub fn new() -> io::Result<Self> {
        // SAFETY: plain socket call; the fd is owned from here on.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd was just opened and nothing else owns it.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: an all-zero sockaddr_nl is valid; only the groups are set.
        let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        addr.nl_family = libc::AF_NETLINK as u16;
        addr.nl_groups = RTMGRP_IPV4_ROUTE;
        // SAFETY: addr is a sockaddr_nl of the length given.
        let bound = unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&raw const addr).cast(),
                size_of::<libc::sockaddr_nl>() as u32,
            )
        };
        if bound < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(Async::new(fd)?))
    }

    /// Waits for the next change and reads every message queued with it.
    pub async fn changed(&self) -> io::Result<()> {
        self.0.readable().await?;
        let mut buf = [0u8; 8192];
        loop {
            // SAFETY: buf is writable for its length.
            let n = unsafe {
                libc::recv(
                    self.0.get_ref().as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                )
            };
            if n < 0 {
                let e = io::Error::last_os_error();
                return match e.kind() {
                    io::ErrorKind::WouldBlock => Ok(()),
                    // Changes came faster than they were read: one look at
                    // the table covers them all.
                    _ if e.raw_os_error() == Some(libc::ENOBUFS) => Ok(()),
                    _ => Err(e),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_attributes() {
        let mut buf = Vec::new();
        attr(&mut buf, IFLA_INFO_KIND, b"rmnet");
        assert_eq!(buf.len(), 12);
        assert_eq!(&buf[..4], &[9, 0, 1, 0]);
        assert_eq!(&buf[4..9], b"rmnet");
    }

    #[test]
    fn reads_a_default_route() {
        let mut body = vec![
            libc::AF_INET as u8,
            0,
            0,
            0,
            RT_TABLE_MAIN,
            RTPROT_STATIC,
            RT_SCOPE_LINK,
            RTN_UNICAST,
        ];
        body.extend_from_slice(&0u32.to_ne_bytes());
        attr(&mut body, RTA_TABLE, &254u32.to_ne_bytes());
        attr(&mut body, RTA_OIF, &7u32.to_ne_bytes());
        attr(&mut body, RTA_PRIORITY, &10000u32.to_ne_bytes());
        assert_eq!(parse_default_route(&body), Some((7, 10000)));
        // A /24 is not a default route.
        body[1] = 24;
        assert_eq!(parse_default_route(&body), None);
    }
}
