//! One request per connection, as systemd's socket hands it over on stdin.
//! PINs never go to argv, the environment or the journal.

use std::io::{self, Write};
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phonewave_gatekeeper::{Outcome, default_store, read_request, reply};

fn boot_ms() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is valid storage for the result.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut time) };
    time.tv_sec as u64 * 1000 + time.tv_nsec as u64 / 1_000_000
}

fn run() -> io::Result<Outcome> {
    // SAFETY: systemd passes the accepted connection as stdin; it is owned here.
    let mut stream = unsafe { UnixStream::from_raw_fd(0) };
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let outcome = match read_request(&mut stream)? {
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
