//! The few rtnetlink requests dmail needs for its rmnet link: create and
//! delete it, bring it up, give it an address and a default route.
//!
//! Requests are blocking: the kernel answers them at once.

use std::cell::Cell;
use std::ffi::CString;
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_NEWADDR: u16 = 20;
const RTM_NEWROUTE: u16 = 24;
const NLMSG_ERROR: u16 = 2;

const NLM_F_REQUEST: u16 = 0x001;
const NLM_F_ACK: u16 = 0x004;
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

    /// Sends one request and waits for the kernel's ack.
    fn request(&self, kind: u16, flags: u16, body: &[u8]) -> io::Result<()> {
        let seq = self.seq.get();
        self.seq.set(seq.wrapping_add(1));
        let mut msg = Vec::with_capacity(16 + body.len());
        msg.extend_from_slice(&((16 + body.len()) as u32).to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        msg.extend_from_slice(&(flags | NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
        msg.extend_from_slice(&seq.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(body);
        // SAFETY: msg is a valid buffer of its length; the kernel is the
        // default destination of an unbound netlink socket.
        if unsafe { libc::send(self.fd.as_raw_fd(), msg.as_ptr().cast(), msg.len(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = [0u8; 8192];
        loop {
            // SAFETY: buf is writable for its length.
            let n =
                unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut rest = &buf[..n as usize];
            while rest.len() >= 20 {
                let len = u32::from_ne_bytes(rest[0..4].try_into().unwrap()) as usize;
                let kind = u16::from_ne_bytes(rest[4..6].try_into().unwrap());
                let got = u32::from_ne_bytes(rest[8..12].try_into().unwrap());
                if kind == NLMSG_ERROR && got == seq {
                    let error = i32::from_ne_bytes(rest[16..20].try_into().unwrap());
                    return match error {
                        0 => Ok(()),
                        e => Err(io::Error::from_raw_os_error(-e)),
                    };
                }
                rest = rest.get(len.next_multiple_of(4)..).unwrap_or_default();
            }
        }
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
}
