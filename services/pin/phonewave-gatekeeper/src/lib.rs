//! The lock PIN behind root, for a phone without a hardware PIN provider.
//!
//! It speaks the hardware providers' socket protocol, `NXGK1`, so Denial
//! keeps a phone's PIN the same way on every phone: Denial's record holds
//! an opaque handle and the PIN's secure ID, and only this worker can check a
//! PIN against it. The PIN's hash, the failure count and the key that signs
//! handles stay in root's state folder.
//!
//! A phone has one PIN. Enrolling a first PIN works only while there is
//! none; after that only an enrollment that proves the current PIN replaces
//! it, and the secure ID stays. A root caller that checks a PIN against the
//! handle in Denial's record therefore checks the PIN the person set, even
//! though the record is the person's own file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// The Gatekeeper user every `NXGK1` provider answers for.
pub const UID: u32 = 941_003;
pub const HEADER_SIZE: usize = 48;
pub const REPLY_SIZE: usize = 40;
pub const HANDLE_MAX: usize = 4096;
pub const SECRET_MAX: usize = 1024;
/// An Android hardware auth token.
pub const TOKEN_SIZE: usize = 69;

const ENROLL: u32 = 1;
const CHANGE: u32 = 2;
const VERIFY: u32 = 3;

const HANDLE_MAGIC: &[u8; 8] = b"PWGK1\0\0\0";
const HANDLE_SIZE: usize = 8 + 8 + 8 + 32;
const KEY_SIZE: usize = 32;
// OWASP's Argon2id baseline, as Denial's own records: 19 MiB, two passes.
const MEMORY_KIB: u32 = 19 * 1024;
const PASSES: u32 = 2;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

type HmacSha256 = Hmac<Sha256>;

pub fn erase(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: a valid, aligned pointer to a byte of the slice.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

/// One request: the header's fields and its three secrets.
pub struct Request {
    pub op: u32,
    pub challenge: u64,
    pub sid: u64,
    pub handle: Vec<u8>,
    pub current: Vec<u8>,
    pub desired: Vec<u8>,
}

impl Drop for Request {
    fn drop(&mut self) {
        erase(&mut self.current);
        erase(&mut self.desired);
    }
}

/// The lengths of the handle, the current PIN and the desired PIN that follow
/// a valid header, or None.
pub fn parse_header(header: &[u8; HEADER_SIZE]) -> Option<(u32, u64, u64, [usize; 3])> {
    let op = u32_at(header, 8);
    let challenge = u64_at(header, 16);
    let sid = u64_at(header, 24);
    let (h, p, n) = (
        u32_at(header, 32) as usize,
        u32_at(header, 36) as usize,
        u32_at(header, 40) as usize,
    );
    let valid = &header[..8] == b"NXGK1\0\0\0"
        && u32_at(header, 12) == 0
        && u32_at(header, 44) == 0
        && h <= HANDLE_MAX
        && p <= SECRET_MAX
        && n <= SECRET_MAX
        && match op {
            ENROLL => h == 0 && p == 0 && n != 0 && challenge == 0 && sid == 0,
            CHANGE => h != 0 && p != 0 && n != 0 && challenge == 0 && sid != 0,
            VERIFY => h != 0 && p != 0 && n == 0 && sid != 0,
            _ => false,
        };
    valid.then_some((op, challenge, sid, [h, p, n]))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A new handle for the PIN, under its secure ID.
    Enrolled { sid: u64, handle: Vec<u8> },
    /// The PIN matched: an auth token for the challenge.
    Verified { sid: u64, token: Vec<u8> },
    /// The PIN did not match.
    Rejected,
    /// Too many wrong PINs: the next try waits this long.
    Throttled(u32),
    /// Refused for this reason, an errno.
    Refused(i32),
}

/// The reply as the hardware providers send it.
pub fn reply(outcome: &Outcome) -> Vec<u8> {
    let (status, service, retry, sid, payload): (i32, i32, u32, u64, &[u8]) = match outcome {
        Outcome::Enrolled { sid, handle } => (0, 0, 0, *sid, handle),
        Outcome::Verified { sid, token } => (0, 0, 0, *sid, token),
        Outcome::Rejected => (-libc::EACCES, -1, 0, 0, &[]),
        Outcome::Throttled(ms) => (-libc::EAGAIN, -2, *ms, 0, &[]),
        Outcome::Refused(errno) => (-errno, 0, 0, 0, &[]),
    };
    let mut bytes = vec![0u8; REPLY_SIZE];
    bytes[..8].copy_from_slice(b"NXGR1\0\0\0");
    bytes[8..12].copy_from_slice(&status.to_le_bytes());
    bytes[12..16].copy_from_slice(&service.to_le_bytes());
    bytes[16..20].copy_from_slice(&retry.to_le_bytes());
    bytes[20..24].copy_from_slice(&UID.to_le_bytes());
    bytes[24..32].copy_from_slice(&sid.to_le_bytes());
    bytes[32..36].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

/// The phone's PIN, as root keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Enrollment {
    sid: u64,
    /// Grows with each change, so an older handle stops working.
    generation: u64,
    hash: String,
    failures: u32,
    /// Wall-clock milliseconds of the last wrong PIN.
    failed_at_ms: u64,
}

/// Android's software Gatekeeper's schedule: a 30 s wait after every fifth
/// wrong PIN up to ten, after each one up to thirty, then doubling every ten.
fn timeout_ms(failures: u32) -> u64 {
    match failures {
        0 => 0,
        1..=10 if failures.is_multiple_of(5) => 30_000,
        1..=10 => 0,
        11..=29 => 30_000,
        30..=139 => 30_000 << ((failures - 30) / 10),
        _ => DAY_MS,
    }
}

impl Enrollment {
    /// How long the next try waits. A clock set back waits the whole time.
    fn remaining_ms(&self, now_ms: u64) -> u64 {
        let timeout = timeout_ms(self.failures);
        if timeout == 0 {
            return 0;
        }
        match now_ms.checked_sub(self.failed_at_ms) {
            Some(elapsed) => timeout.saturating_sub(elapsed),
            None => timeout,
        }
    }
}

fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    let mut filled = 0;
    while filled < N {
        // SAFETY: the pointer and length describe the unfilled part of `bytes`.
        let count = unsafe { libc::getrandom(bytes[filled..].as_mut_ptr().cast(), N - filled, 0) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
            continue;
        }
        filled += count as usize;
    }
    Ok(bytes)
}

fn hash(pin: &[u8]) -> io::Result<String> {
    let failed = |error: &dyn std::fmt::Display| io::Error::other(error.to_string());
    let salt = SaltString::encode_b64(&random::<16>()?).map_err(|error| failed(&error))?;
    let params = Params::new(MEMORY_KIB, PASSES, 1, None).map_err(|error| failed(&error))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password(pin, &salt)
        .map_err(|error| failed(&error))?
        .to_string())
}

fn matches(hash: &str, pin: &[u8]) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| Argon2::default().verify_password(pin, &hash).is_ok())
}

/// Root's state folder.
pub struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    /// Held for the whole request: requests come one connection each.
    fn lock(&self) -> io::Result<File> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.directory)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.path("lock"))?;
        // SAFETY: a valid descriptor; flock takes scalar arguments.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }

    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.path(name))
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(65536)
            .read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    /// Writes a sibling and renames it over `name`, so a crash keeps the old one.
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let staging = self.path(&format!(".{name}.new"));
        let _ = fs::remove_file(&staging);
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&staging)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&staging, self.path(name))?;
            File::open(&self.directory)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&staging);
        }
        result
    }

    fn key(&self) -> io::Result<[u8; KEY_SIZE]> {
        if let Some(mut bytes) = self.read("key")? {
            let key = <[u8; KEY_SIZE]>::try_from(bytes.as_slice())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid key"));
            erase(&mut bytes);
            return key;
        }
        let key = random::<KEY_SIZE>()?;
        self.write("key", &key)?;
        Ok(key)
    }

    fn enrollment(&self) -> io::Result<Option<Enrollment>> {
        self.read("enrollment")?
            .map(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid enrollment"))
            })
            .transpose()
    }

    fn save(&self, enrollment: &Enrollment) -> io::Result<()> {
        self.write("enrollment", &serde_json::to_vec(enrollment)?)
    }

    /// Answers one request. `now_ms` is wall-clock time, for the waits;
    /// `boot_ms` is the token's timestamp.
    pub fn handle(&self, request: &Request, now_ms: u64, boot_ms: u64) -> io::Result<Outcome> {
        let _lock = self.lock()?;
        let key = self.key()?;
        let enrollment = self.enrollment()?;
        if request.op == ENROLL {
            if enrollment.is_some() {
                return Ok(Outcome::Refused(libc::EEXIST));
            }
            let sid = loop {
                let sid = u64::from_le_bytes(random::<8>()?);
                if sid != 0 {
                    break sid;
                }
            };
            let enrollment = Enrollment {
                sid,
                generation: 1,
                hash: hash(&request.desired)?,
                failures: 0,
                failed_at_ms: 0,
            };
            self.save(&enrollment)?;
            return Ok(Outcome::Enrolled {
                sid,
                handle: handle(&key, &enrollment),
            });
        }
        let Some(mut enrollment) = enrollment else {
            return Ok(Outcome::Refused(libc::ENOENT));
        };
        if request.sid != enrollment.sid || !current(&key, &enrollment, &request.handle) {
            return Ok(Outcome::Refused(libc::ESTALE));
        }
        let waiting = enrollment.remaining_ms(now_ms);
        if waiting > 0 {
            return Ok(Outcome::Throttled(waiting.min(u64::from(u32::MAX)) as u32));
        }
        if !matches(&enrollment.hash, &request.current) {
            enrollment.failures = enrollment.failures.saturating_add(1);
            enrollment.failed_at_ms = now_ms;
            self.save(&enrollment)?;
            return Ok(Outcome::Rejected);
        }
        if request.op == CHANGE {
            enrollment.generation += 1;
            enrollment.hash = hash(&request.desired)?;
        }
        if enrollment.failures != 0 || request.op == CHANGE {
            enrollment.failures = 0;
            enrollment.failed_at_ms = 0;
            self.save(&enrollment)?;
        }
        Ok(if request.op == CHANGE {
            Outcome::Enrolled {
                sid: enrollment.sid,
                handle: handle(&key, &enrollment),
            }
        } else {
            Outcome::Verified {
                sid: enrollment.sid,
                token: token(&key, request.challenge, enrollment.sid, boot_ms),
            }
        })
    }
}

fn mac(key: &[u8], label: &[u8], bytes: &[u8]) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key");
    mac.update(label);
    mac.update(bytes);
    mac
}

fn handle(key: &[u8], enrollment: &Enrollment) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HANDLE_SIZE);
    bytes.extend_from_slice(HANDLE_MAGIC);
    bytes.extend_from_slice(&enrollment.sid.to_le_bytes());
    bytes.extend_from_slice(&enrollment.generation.to_le_bytes());
    let tag = mac(key, b"handle", &bytes).finalize().into_bytes();
    bytes.extend_from_slice(&tag);
    bytes
}

/// Whether `bytes` is the handle of the PIN as it is now.
fn current(key: &[u8], enrollment: &Enrollment, bytes: &[u8]) -> bool {
    bytes.len() == HANDLE_SIZE
        && &bytes[..8] == HANDLE_MAGIC
        && u64_at(bytes, 8) == enrollment.sid
        && u64_at(bytes, 16) == enrollment.generation
        && mac(key, b"handle", &bytes[..24])
            .verify_slice(&bytes[24..])
            .is_ok()
}

/// A password-type Android hardware auth token, signed with the worker's key.
fn token(key: &[u8], challenge: u64, sid: u64, boot_ms: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(TOKEN_SIZE);
    bytes.push(0);
    bytes.extend_from_slice(&challenge.to_le_bytes());
    bytes.extend_from_slice(&sid.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&boot_ms.to_be_bytes());
    let tag = mac(key, b"token", &bytes).finalize().into_bytes();
    bytes.extend_from_slice(&tag);
    bytes
}

/// Reads one request from `stream`.
pub fn read_request(stream: &mut impl Read) -> io::Result<Option<Request>> {
    let mut header = [0u8; HEADER_SIZE];
    stream.read_exact(&mut header)?;
    let Some((op, challenge, sid, [h, p, n])) = parse_header(&header) else {
        return Ok(None);
    };
    let mut request = Request {
        op,
        challenge,
        sid,
        handle: vec![0; h],
        current: vec![0; p],
        desired: vec![0; n],
    };
    stream.read_exact(&mut request.handle)?;
    stream.read_exact(&mut request.current)?;
    stream.read_exact(&mut request.desired)?;
    Ok(Some(request))
}

/// The default state folder, `StateDirectory=` of the unit.
pub fn default_store() -> Store {
    Store::at(Path::new("/var/lib/phonewave-gatekeeper"))
}

#[cfg(test)]
mod tests;
