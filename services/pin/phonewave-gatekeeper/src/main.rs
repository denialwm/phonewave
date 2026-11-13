//! One request per connection, as systemd's socket hands it over on stdin.
//! PINs never go to argv, the environment or the journal.

mod pam;

use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phonewave_gatekeeper::{Outcome, RESET, Request, default_store, read_request, reply};

fn boot_ms() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is valid storage for the result.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut time) };
    time.tv_sec as u64 * 1000 + time.tv_nsec as u64 / 1_000_000
}

/// The account on the other end of the connection.
fn peer(stream: &UnixStream) -> io::Result<CString> {
    // SAFETY: ucred is plain data.
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: peer and size describe valid writable storage.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut peer).cast(),
            &raw mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: passwd is plain data.
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16384];
    // SAFETY: record, buffer and result stay valid through the call.
    let failed = unsafe {
        libc::getpwuid_r(
            peer.uid,
            &raw mut record,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut result,
        )
    };
    if failed != 0 || result.is_null() || record.pw_name.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the caller has no account",
        ));
    }
    // SAFETY: getpwuid_r filled a NUL-terminated name into `buffer`.
    Ok(unsafe { CStr::from_ptr(record.pw_name) }.to_owned())
}

/// A reset sets the PIN only for a caller whose account password root has
/// just checked; a wrong password counts as a wrong PIN, and PAM's own
/// lockout throttles it.
fn reset(stream: &UnixStream, request: &Request) -> io::Result<Outcome> {
    let user = peer(stream)?;
    match pam::check(&user, &request.current) {
        Ok(true) => default_store().reset(request),
        Ok(false) => Ok(Outcome::Rejected),
        Err(error) => {
            eprintln!("phonewave-gatekeeper: {error}");
            Ok(Outcome::Refused(libc::EIO))
        }
    }
}

fn run() -> io::Result<Outcome> {
    // SAFETY: systemd passes the accepted connection as stdin; it is owned here.
    let mut stream = unsafe { UnixStream::from_raw_fd(0) };
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let outcome = match read_request(&mut stream)? {
        Some(request) if request.op == RESET => reset(&stream, &request)?,
        Some(request) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64);
            default_store().handle(&request, now, boot_ms())?
        }
        None => Outcome::Refused(libc::EINVAL),
    };
    let mut bytes = reply(&outcome);
    let written = stream.write_all(&bytes);
    phonewave_gatekeeper::erase(&mut bytes);
    written?;
    Ok(outcome)
}

fn main() {
    // SAFETY: geteuid and prctl take scalar arguments.
    if unsafe { libc::geteuid() } != 0 || unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) } != 0 {
        eprintln!("phonewave-gatekeeper: must run as root");
        std::process::exit(2);
    }
    match run() {
        Ok(outcome) => {
            let name = match outcome {
                Outcome::Enrolled { .. } => "enrolled",
                Outcome::Verified { .. } => "verified",
                Outcome::Rejected => "rejected",
                Outcome::Throttled(_) => "throttled",
                Outcome::Refused(_) => "refused",
            };
            eprintln!("phonewave-gatekeeper: {name}");
        }
        Err(error) => {
            eprintln!("phonewave-gatekeeper: {error}");
            std::process::exit(1);
        }
    }
}
