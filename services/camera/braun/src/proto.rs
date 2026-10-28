//! The session between braun and one client, over the socket `Connect`
//! returns: SOCK_SEQPACKET, one message per packet, little-endian, its
//! first byte its kind. File descriptors travel beside a message. Previews
//! are in memory braun shares when a camera opens, in slots the client
//! hands back once it has read them.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

/// What a client asks.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Open camera `id`, closing the one open.
    Open(String),
    Close,
    /// Focus on this point of the preview, 0 to 1 across and down.
    Focus(f32, f32),
    /// Crop the middle `1 / zoom` of the frame, in the preview, its
    /// metering and the photos.
    Zoom(f32),
    /// Take photo `shot`. The JPEG goes to the first descriptor; with three,
    /// the raw frame the preview showed goes to the second and its notes to
    /// the third.
    Shoot(u32),
    /// The client has read the frame in this slot.
    Release(u32),
}

/// What braun says.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// The camera is open. Frames come in `slots` slots of `slot_bytes` each
    /// in the memory beside this message; the camera's frame is `width` x
    /// `height` upright, and it can focus if it has a `lens`.
    Opened {
        width: u32,
        height: u32,
        lens: bool,
        slots: u32,
        slot_bytes: u32,
    },
    /// A preview in `slot`: BGRA, `width` x `height`, rows packed.
    Frame {
        slot: u32,
        width: u32,
        height: u32,
        sequence: u64,
    },
    /// Photo `shot`'s burst is in: it is being developed.
    Captured(u32),
    /// Photo `shot` is written.
    Saved(u32),
    /// Photo `shot` failed, or, for 0, the camera did: it is closed.
    Failed(u32, String),
}

/// The most descriptors beside one message.
pub const MAX_FDS: usize = 3;
const MAX_MESSAGE: usize = 4096;

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.0.split_at_checked(N)?;
        self.0 = rest;
        head.try_into().ok()
    }
    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take().map(f32::from_le_bytes)
    }
    fn rest(&mut self) -> String {
        String::from_utf8_lossy(std::mem::take(&mut self.0)).into_owned()
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Request::Open(id) => {
                out.push(1);
                out.extend_from_slice(id.as_bytes());
            }
            Request::Close => out.push(2),
            Request::Focus(x, y) => {
                out.push(3);
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
            }
            Request::Zoom(zoom) => {
                out.push(4);
                out.extend_from_slice(&zoom.to_le_bytes());
            }
            Request::Shoot(shot) => {
                out.push(5);
                out.extend_from_slice(&shot.to_le_bytes());
            }
            Request::Release(slot) => {
                out.push(6);
                out.extend_from_slice(&slot.to_le_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&kind, rest) = bytes.split_first()?;
        let mut reader = Reader(rest);
        Some(match kind {
            1 => Request::Open(reader.rest()),
            2 => Request::Close,
            3 => Request::Focus(reader.f32()?, reader.f32()?),
            4 => Request::Zoom(reader.f32()?),
            5 => Request::Shoot(reader.u32()?),
            6 => Request::Release(reader.u32()?),
            _ => return None,
        })
    }
}

impl Reply {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Reply::Opened {
                width,
                height,
                lens,
                slots,
                slot_bytes,
            } => {
                out.push(1);
                out.extend_from_slice(&width.to_le_bytes());
                out.extend_from_slice(&height.to_le_bytes());
                out.push(*lens as u8);
                out.extend_from_slice(&slots.to_le_bytes());
                out.extend_from_slice(&slot_bytes.to_le_bytes());
            }
            Reply::Frame {
                slot,
                width,
                height,
                sequence,
            } => {
                out.push(2);
                out.extend_from_slice(&slot.to_le_bytes());
                out.extend_from_slice(&width.to_le_bytes());
                out.extend_from_slice(&height.to_le_bytes());
                out.extend_from_slice(&sequence.to_le_bytes());
            }
            Reply::Captured(shot) => {
                out.push(3);
                out.extend_from_slice(&shot.to_le_bytes());
            }
            Reply::Saved(shot) => {
                out.push(4);
                out.extend_from_slice(&shot.to_le_bytes());
            }
            Reply::Failed(shot, text) => {
                out.push(5);
                out.extend_from_slice(&shot.to_le_bytes());
                // A message is one packet: keep the text within it.
                let mut end = text.len().min(MAX_MESSAGE - 8);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                out.extend_from_slice(&text.as_bytes()[..end]);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&kind, rest) = bytes.split_first()?;
        let mut reader = Reader(rest);
        Some(match kind {
            1 => Reply::Opened {
                width: reader.u32()?,
                height: reader.u32()?,
                lens: reader.take::<1>()?[0] != 0,
                slots: reader.u32()?,
                slot_bytes: reader.u32()?,
            },
            2 => Reply::Frame {
                slot: reader.u32()?,
                width: reader.u32()?,
                height: reader.u32()?,
                sequence: reader.u64()?,
            },
            3 => Reply::Captured(reader.u32()?),
            4 => Reply::Saved(reader.u32()?),
            5 => Reply::Failed(reader.u32()?, reader.rest()),
            _ => return None,
        })
    }
}

/// A connected pair of session sockets.
pub fn pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: socketpair fills both descriptors on success.
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both are new descriptors this process owns.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Room for `MAX_FDS` descriptors of ancillary data, aligned for cmsghdr.
#[repr(C, align(8))]
struct Control([u8; 64]);

/// Sends one message with `fds` beside it.
pub fn send(socket: BorrowedFd<'_>, bytes: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    assert!(fds.len() <= MAX_FDS);
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    let mut control = Control([0; 64]);
    // SAFETY: an all-zero msghdr is valid; the pointers set below outlive
    // the call.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    if !fds.is_empty() {
        let size = std::mem::size_of_val(fds) as u32;
        // SAFETY: the control buffer holds one header and `MAX_FDS` fds.
        unsafe {
            message.msg_control = control.0.as_mut_ptr().cast();
            message.msg_controllen = libc::CMSG_SPACE(size) as _;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(size) as _;
            let data = libc::CMSG_DATA(header).cast::<RawFd>();
            for (index, fd) in fds.iter().enumerate() {
                data.add(index).write_unaligned(fd.as_raw_fd());
            }
        }
    }
    loop {
        // SAFETY: `message` describes valid buffers.
        let sent = unsafe { libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
        if sent >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Takes the next message and the descriptors beside it. Without `wait`,
/// `None` when no message is there. The peer leaving is `UnexpectedEof`.
pub fn recv(socket: BorrowedFd<'_>, wait: bool) -> io::Result<Option<(Vec<u8>, Vec<OwnedFd>)>> {
    let mut bytes = vec![0u8; MAX_MESSAGE];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    let mut control = Control([0; 64]);
    // SAFETY: an all-zero msghdr is valid; the pointers set below outlive
    // the call.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.0.as_mut_ptr().cast();
    message.msg_controllen = control.0.len() as _;
    let flags = libc::MSG_CMSG_CLOEXEC | if wait { 0 } else { libc::MSG_DONTWAIT };
    let length = loop {
        // SAFETY: `message` describes valid buffers.
        let length = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, flags) };
        if length >= 0 {
            break length as usize;
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock if !wait => return Ok(None),
            _ => return Err(error),
        }
    };
    let mut fds = Vec::new();
    // SAFETY: walks the control messages recvmsg wrote.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                let count =
                    ((*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / size_of::<RawFd>();
                for index in 0..count {
                    fds.push(OwnedFd::from_raw_fd(data.add(index).read_unaligned()));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    if length == 0 && fds.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the other side left",
        ));
    }
    bytes.truncate(length);
    Ok(Some((bytes, fds)))
}

/// Memory shared between braun and its client: the preview slots.
pub struct Shared {
    data: *mut u8,
    length: usize,
    fd: OwnedFd,
}

// The memory is plain bytes; who writes where is the protocol's business.
unsafe impl Send for Shared {}

impl Shared {
    /// New memory of `length` bytes, to write.
    pub fn create(length: usize) -> io::Result<Self> {
        // SAFETY: the name is a C string; the flags are memfd's own.
        let raw = unsafe {
            libc::memfd_create(
                c"braun-preview".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new descriptor this process owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: sizes the memfd this process owns.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), length as libc::off_t) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // The client can neither shrink nor grow it under braun.
        // SAFETY: seals the memfd this process owns.
        unsafe {
            libc::fcntl(
                fd.as_raw_fd(),
                libc::F_ADD_SEALS,
                libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL,
            )
        };
        Self::map(fd, length, libc::PROT_READ | libc::PROT_WRITE)
    }

    /// The client's view of memory braun shared, `length` bytes, to read.
    pub fn open(fd: OwnedFd, length: usize) -> io::Result<Self> {
        Self::map(fd, length, libc::PROT_READ)
    }

    fn map(fd: OwnedFd, length: usize, protection: i32) -> io::Result<Self> {
        // SAFETY: maps `length` bytes of the memfd.
        let data = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                protection,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if data == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            data: data.cast(),
            length,
            fd,
        })
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.fd.as_fd()
    }

    /// `length` bytes from `offset`, if the memory has them.
    pub fn read(&self, offset: usize, length: usize) -> Option<&[u8]> {
        (offset.checked_add(length)? <= self.length)
            // SAFETY: within the mapping.
            .then(|| unsafe { std::slice::from_raw_parts(self.data.add(offset), length) })
    }

    /// Writes `bytes` at `offset`, if they fit.
    pub fn write(&mut self, offset: usize, bytes: &[u8]) -> bool {
        let fits = offset
            .checked_add(bytes.len())
            .is_some_and(|end| end <= self.length);
        if fits {
            // SAFETY: within the mapping, which braun maps writable.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.data.add(offset), bytes.len())
            };
        }
        fits
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: mapped by `map` with this length.
        unsafe { libc::munmap(self.data.cast(), self.length) };
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use super::{Reply, Request, Shared, pair, recv, send};

    #[test]
    fn messages_round_trip() {
        for request in [
            Request::Open("ov50e40".into()),
            Request::Close,
            Request::Focus(0.25, 0.75),
            Request::Zoom(2.0),
            Request::Shoot(7),
            Request::Release(2),
        ] {
            assert_eq!(Request::decode(&request.encode()), Some(request));
        }
        for reply in [
            Reply::Opened {
                width: 3072,
                height: 4096,
                lens: true,
                slots: 3,
                slot_bytes: 3 << 20,
            },
            Reply::Frame {
                slot: 1,
                width: 768,
                height: 1024,
                sequence: 1 << 40,
            },
            Reply::Captured(3),
            Reply::Saved(3),
            Reply::Failed(0, "OV50E40: no frame from the sensor".into()),
        ] {
            assert_eq!(Reply::decode(&reply.encode()), Some(reply));
        }
    }

    #[test]
    fn descriptors_travel_with_messages() {
        let (a, b) = pair().unwrap();
        let mut shared = Shared::create(4096).unwrap();
        assert!(shared.write(10, b"frame"));
        assert!(!shared.write(4094, b"frame"));
        send(a.as_fd(), &Request::Shoot(1).encode(), &[shared.fd()]).unwrap();
        let (bytes, fds) = recv(b.as_fd(), true).unwrap().unwrap();
        assert_eq!(Request::decode(&bytes), Some(Request::Shoot(1)));
        let view = Shared::open(fds.into_iter().next().unwrap(), 4096).unwrap();
        assert_eq!(view.read(10, 5), Some(&b"frame"[..]));
        assert!(recv(b.as_fd(), false).unwrap().is_none());
        drop(a);
        assert!(recv(b.as_fd(), true).is_err());
    }
}
