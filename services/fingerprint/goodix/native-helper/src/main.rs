// SPDX-License-Identifier: LGPL-2.1-or-later
//! NX809J fprintd bridge. Contact and trusted operations are separate children.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// A verification without a match leaves its worker warm; the next
/// attempt's bridge connects here (root-only directory).
const WARM_SOCKET: &str = "/run/nx809j-fingerprint/verify.sock";

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
type Result<T> = std::result::Result<T, String>;
fn fail<T>(message: &str) -> Result<T> {
    Err(message.into())
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        fail(message)
    }
}
fn emit(line: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "UFF1 {line}")
        .and_then(|_| out.flush())
        .map_err(|_| "output closed".into())
}
fn protected(path: &Path, limit: u64) -> Result<Vec<u8>> {
    require(path.is_absolute(), "absolute artifact path required")?;
    for ancestor in path.ancestors().skip(1) {
        let m = fs::symlink_metadata(ancestor).map_err(|_| "artifact parent unavailable")?;
        require(
            m.is_dir() && m.uid() == 0 && m.mode() & 0o022 == 0,
            "unprotected artifact directory",
        )?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "artifact unavailable")?;
    let m = file
        .metadata()
        .map_err(|_| "artifact metadata unavailable")?;
    require(
        m.is_file()
            && m.uid() == 0
            && m.nlink() == 1
            && m.mode() & 0o022 == 0
            && m.len() > 0
            && m.len() <= limit,
        "unprotected artifact",
    )?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "artifact read failed")?;
    require(
        bytes.len() as u64 == m.len(),
        "artifact changed while reading",
    )?;
    Ok(bytes)
}
#[derive(Clone)]
struct Config {
    worker: PathBuf,
    firmware: PathBuf,
    contact: PathBuf,
}
fn configuration() -> Result<Config> {
    require(unsafe { libc::geteuid() } == 0, "root required")?;
    let board = fs::read("/proc/device-tree/compatible").map_err(|_| "board unavailable")?;
    require(
        board.split(|b| *b == 0).next() == Some(b"nubia,nx809j"),
        "unsupported board",
    )?;
    let boot =
        fs::read_to_string("/proc/bootconfig").map_err(|_| "boot configuration unavailable")?;
    require(
        boot.contains("androidboot.slot_suffix = \"_b\""),
        "wrong slot",
    )?;
    let v: Value = serde_json::from_slice(&protected(
        Path::new("/etc/nx809j-fingerprint/backend.json"),
        8192,
    )?)
    .map_err(|_| "invalid configuration")?;
    require(v["format"].as_u64() == Some(1), "unsupported configuration")?;
    fn artifact(v: &Value, key: &str) -> Result<PathBuf> {
        let path = PathBuf::from(v[key]["path"].as_str().ok_or("missing artifact path")?);
        let digest = v[key]["sha256"].as_str().ok_or("missing artifact hash")?;
        require(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "invalid artifact hash",
        )?;
        let bytes = protected(&path, 32 * 1024 * 1024)?;
        require(
            format!("{:x}", Sha256::digest(&bytes)) == digest,
            "artifact checksum mismatch",
        )?;
        Ok(path)
    }
    Ok(Config {
        worker: artifact(&v, "worker")?,
        firmware: artifact(&v, "firmware")?,
        contact: artifact(&v, "contact")?,
    })
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CState {
    Starting,
    Waiting,
    Down,
    Presenting,
    Ready,
    Finishing,
}
struct Contact {
    state: CState,
    stopped: bool,
    /* Released once the image is taken: a lift after that ends the
     * contact as cleanly as a finish does. */
    released: bool,
    lifted: bool,
    suspended: bool,
    pending: Option<bool>,
}
impl Contact {
    fn new() -> Self {
        Self {
            state: CState::Starting,
            stopped: false,
            released: false,
            lifted: false,
            suspended: false,
            pending: None,
        }
    }
    fn record(&mut self, line: &str) -> Result<()> {
        match line {
            "DFP1 waiting" if self.state == CState::Starting => self.state = CState::Waiting,
            "DFP1 down 1" if self.state == CState::Waiting && !self.suspended => {
                self.state = CState::Down
            }
            "DFP1 ready 1" if self.state == CState::Presenting => self.state = CState::Ready,
            "DFP1 stopped 1 0" if self.state == CState::Finishing && !self.stopped => {
                self.stopped = true
            }
            "DFP1 lost 1" if self.state == CState::Finishing && self.released && !self.stopped => {
                self.stopped = true;
                self.lifted = true;
            }
            "DFP1 suspended" if self.pending == Some(true) && self.state == CState::Waiting => {
                self.pending = None;
                self.suspended = true;
                emit("suspended")?;
            }
            "DFP1 resumed" if self.pending == Some(false) && self.state == CState::Waiting => {
                self.pending = None;
                self.suspended = false;
                emit("resumed")?;
            }
            "DFP1 suspend-busy" if self.pending == Some(true) => {
                self.pending = None;
                emit("suspend-busy")?;
            }
            _ => return fail("contact cancelled or invalid transition"),
        }
        Ok(())
    }
    fn complete(&self, success: bool) -> Result<()> {
        require(
            self.state == CState::Finishing && self.stopped && (success || self.lifted),
            "contact cleanup unconfirmed",
        )
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NState {
    Starting,
    Standby,
    Activating,
    Prepared,
    Capturing,
    Sample,
    Finishing,
    Resting,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Sample {
    code: u32,
    remaining: u32,
    id: u32,
    total: u32,
}
struct Native {
    operation: String,
    state: NState,
    /* The worker has the image; the finger is no longer needed. */
    captured: bool,
    /* Told to rest rather than finish: it stays up for the next attempt. */
    rest: bool,
    cleaned: bool,
    sample: Option<Sample>,
    result: Option<(u32, u32)>,
    ids: Vec<u32>,
}
impl Native {
    fn new(operation: &str) -> Self {
        Self {
            operation: operation.into(),
            state: NState::Starting,
            captured: false,
            rest: false,
            cleaned: false,
            sample: None,
            result: None,
            ids: Vec::new(),
        }
    }
    fn record(&mut self, line: &str) -> Result<()> {
        require(self.result.is_none(), "record after native result")?;
        let parts: Vec<&str> = line.split(' ').collect();
        match parts.as_slice() {
            ["UFW1", "standby"] if self.state == NState::Starting && self.operation == "verify" => {
                self.state = NState::Standby
            }
            ["UFW1", "prepared"]
                if (self.operation == "enroll" && self.state == NState::Starting)
                    || (self.operation == "verify" && self.state == NState::Activating) =>
            {
                self.state = NState::Prepared
            }
            ["UFW1", "captured"] if self.state == NState::Capturing && !self.captured => {
                self.captured = true
            }
            ["UFW1", "sample", code, remaining, id, total] if self.state == NState::Capturing => {
                let s = Sample {
                    code: number(code)?,
                    remaining: number(remaining)?,
                    id: number(id)?,
                    total: number(total)?,
                };
                require(
                    matches!(s.code, 0 | 1 | 3) && s.remaining <= 100 && s.total <= 101,
                    "invalid sample",
                )?;
                if self.operation == "verify" {
                    require(
                        (s.code == 0 && s.id != 0) || (s.code != 0 && s.id == 0),
                        "invalid verification identifier",
                    )?;
                } else {
                    require(
                        s.code != 1 && (s.code != 0 || s.total > 0),
                        "invalid enrollment sample",
                    )?;
                }
                self.sample = Some(s);
                self.state = NState::Sample;
            }
            ["UFW1", "resting"] if self.state == NState::Finishing && self.rest => {
                self.state = NState::Resting
            }
            ["UFW1", "idle"] if self.state == NState::Finishing && self.operation == "enroll" => {
                let s = self.sample.ok_or("missing sample")?;
                require(s.code != 0 || s.remaining != 0, "idle after final sample")?;
                self.state = NState::Prepared;
            }
            ["UFW1", "cleaned"]
                if !self.cleaned && matches!(self.operation.as_str(), "enroll" | "verify") =>
            {
                self.cleaned = true
            }
            ["UFF1", "item", id] if self.operation == "list" => {
                let id = number(id)?;
                require(
                    id != 0 && !self.ids.contains(&id) && self.ids.len() < 10,
                    "invalid template identifier",
                )?;
                self.ids.push(id);
            }
            ["UFF1", "result", code, id] => {
                let code = number(code)?;
                let id = number(id)?;
                require(matches!(code, 0 | 1 | 3), "invalid result")?;
                if matches!(self.operation.as_str(), "verify" | "enroll") {
                    let s = self.sample.ok_or("missing sample")?;
                    require(
                        self.cleaned
                            && self.state == NState::Finishing
                            && (code, id) == (s.code, s.id),
                        "result before confirmed cleanup",
                    )?;
                    if self.operation == "enroll" {
                        require(
                            code == 0 && id != 0 && s.remaining == 0,
                            "incomplete enrollment",
                        )?;
                    }
                } else {
                    require(code == 0 && id == 0, "unexpected passive result")?;
                }
                self.result = Some((code, id));
            }
            _ => return fail("invalid native record or transition"),
        }
        Ok(())
    }
    fn complete(&self, success: bool) -> Result<(u32, u32)> {
        require(
            success && self.result.is_some(),
            "native cleanup or operation failed",
        )?;
        Ok(self.result.unwrap())
    }
}
fn number(s: &str) -> Result<u32> {
    require(
        !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()),
        "invalid integer",
    )?;
    s.parse().map_err(|_| "integer out of range".into())
}
struct Process {
    /// None for a warm worker adopted from an earlier bridge.
    child: Option<Child>,
    /// Readable once the process has exited; for an adopted worker only
    /// the last resort to end it.
    pidfd: Option<OwnedFd>,
    input: File,
    output: File,
    buffer: Vec<u8>,
    bytes: usize,
    eof: bool,
    status: Option<bool>,
}
fn pidfd(pid: i32) -> Option<OwnedFd> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd as i32) })
}
fn readable(fd: i32, ms: i32) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    (unsafe { libc::poll(&mut p, 1, ms) }) == 1
}
/// The warm worker, if one is listening: root, as its directory demands.
fn connect_warm(path: &Path) -> Option<(UnixStream, i32)> {
    let stream = UnixStream::connect(path).ok()?;
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut peer as *mut libc::ucred).cast(),
            &mut size,
        )
    } == 0
        && size as usize == std::mem::size_of::<libc::ucred>()
        && peer.uid == unsafe { libc::geteuid() };
    ok.then_some((stream, peer.pid))
}
impl Process {
    fn spawn(mut cmd: Command) -> Result<Self> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().map_err(|_| "child start failed")?;
        let input = child.stdin.take().ok_or("missing child input")?;
        let output = child.stdout.take().ok_or("missing child output")?;
        Ok(Self {
            pidfd: pidfd(child.id() as i32),
            child: Some(child),
            input: File::from(OwnedFd::from(input)),
            output: File::from(OwnedFd::from(output)),
            buffer: Vec::new(),
            bytes: 0,
            eof: false,
            status: None,
        })
    }
    fn adopt((stream, pid): (UnixStream, i32)) -> Result<Self> {
        let input = stream.try_clone().map_err(|_| "warm worker unavailable")?;
        Ok(Self {
            child: None,
            pidfd: Some(pidfd(pid).ok_or("warm worker unavailable")?),
            input: File::from(OwnedFd::from(input)),
            output: File::from(OwnedFd::from(stream)),
            buffer: Vec::new(),
            bytes: 0,
            eof: false,
            status: None,
        })
    }
    fn exit_fd(&self) -> i32 {
        match (&self.child, &self.pidfd, self.status) {
            (Some(_), Some(fd), None) => fd.as_raw_fd(),
            _ => -1,
        }
    }
    fn send(&mut self, line: &str) -> Result<()> {
        writeln!(self.input, "{line}")
            .and_then(|_| self.input.flush())
            .map_err(|_| "child control closed".into())
    }
    fn poll_exit(&mut self) -> Result<()> {
        if self.status.is_none() {
            match self.child.as_mut() {
                Some(child) => {
                    if let Some(s) = child.try_wait().map_err(|_| "child wait failed")? {
                        self.status = Some(s.success());
                    }
                }
                // Not our child: it holds its stream until it exits, and
                // its records alone say how it ended.
                None => {
                    if self.eof {
                        self.status = Some(true);
                    }
                }
            }
        }
        Ok(())
    }
    fn joined(&mut self) -> Result<bool> {
        self.poll_exit()?;
        Ok(self.eof && self.status.is_some())
    }
    fn stop(&mut self) -> Result<()> {
        self.poll_exit()?;
        if self.status.is_some() {
            return Ok(());
        }
        match &self.child {
            Some(child) => unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            },
            // An adopted worker cancels at the end of its control stream.
            None => unsafe {
                libc::shutdown(self.input.as_raw_fd(), libc::SHUT_WR);
            },
        }
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            self.poll_exit()?;
            if self.status.is_some() {
                return Ok(());
            }
            if Instant::now() >= end {
                match self.child.as_mut() {
                    Some(child) => {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    None => unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            self.pidfd.as_ref().unwrap().as_raw_fd(),
                            libc::SIGKILL,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        );
                    },
                }
                return fail("forced child termination; cleanup unconfirmed");
            }
            if self.child.is_some() {
                readable(self.pidfd.as_ref().map_or(-1, |fd| fd.as_raw_fd()), 20);
            } else if readable(self.output.as_raw_fd(), 20) {
                let mut b = [0u8; 1024];
                self.eof = !matches!(self.output.read(&mut b), Ok(n) if n > 0);
            }
        }
    }
}
struct Bridge {
    config: Config,
    operation: String,
    argument: Option<String>,
    native: Option<Process>,
    contact: Option<Process>,
    n: Native,
    c: Contact,
    control: Vec<u8>,
    control_fd: i32,
    touch_at: Option<Instant>,
    warm: PathBuf,
}
impl Bridge {
    fn new(config: Config, operation: &str, argument: Option<String>) -> Self {
        Self {
            config,
            operation: operation.into(),
            argument,
            native: None,
            contact: None,
            n: Native::new(operation),
            c: Contact::new(),
            control: Vec::new(),
            control_fd: 0,
            touch_at: None,
            warm: PathBuf::from(WARM_SOCKET),
        }
    }
    fn start_native(&mut self) -> Result<()> {
        // A verification takes over a warm worker; enrollment, listing and
        // deletion need the sensor to themselves, so they stop one first.
        // Probe and open only look, and accept a warm worker's power.
        match self.operation.as_str() {
            "verify" => {
                if let Some(warm) = connect_warm(&self.warm) {
                    self.native = Some(Process::adopt(warm)?);
                    return Ok(());
                }
            }
            "probe" | "open" => {}
            _ => {
                if let Some(warm) = connect_warm(&self.warm) {
                    Process::adopt(warm)?.stop()?;
                }
            }
        }
        let mut c = Command::new(&self.config.worker);
        c.arg(&self.operation).arg(&self.config.firmware);
        if let Some(a) = &self.argument {
            c.arg(a);
        }
        self.native = Some(Process::spawn(c)?);
        Ok(())
    }
    fn start_contact(&mut self) -> Result<()> {
        let mut c = Command::new(&self.config.contact);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("UFF_SUSPEND_PROTOCOL", "1");
        self.contact = Some(Process::spawn(c)?);
        self.c = Contact::new();
        Ok(())
    }
    fn control(&mut self, line: &str) -> Result<()> {
        require(
            line == "suspend" || line == "resume",
            "invalid service control request",
        )?;
        let p = self.contact.as_mut();
        if self.operation != "verify"
            || self.n.state != NState::Standby
            || p.is_none()
            || p.as_ref().unwrap().eof
            || self.c.state != CState::Waiting
        {
            require(line == "suspend", "resume while capture active")?;
            return emit("suspend-busy");
        }
        require(
            self.c.pending.is_none() && (line == "resume") == self.c.suspended,
            "invalid sleep transition",
        )?;
        self.c.pending = Some(line == "suspend");
        p.unwrap().send(line)
    }
    fn pump(&mut self, timeout: Duration) -> Result<()> {
        require(!STOP.load(Ordering::Relaxed), "cancelled")?;
        let mut fds = [
            libc::pollfd {
                fd: self
                    .contact
                    .as_ref()
                    .filter(|p| !p.eof)
                    .map_or(-1, |p| p.output.as_raw_fd()),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.control_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self
                    .native
                    .as_ref()
                    .filter(|p| !p.eof)
                    .map_or(-1, |p| p.output.as_raw_fd()),
                events: libc::POLLIN,
                revents: 0,
            },
            // A child's exit wakes the poll too, so its join is seen at once.
            libc::pollfd {
                fd: self.contact.as_ref().map_or(-1, Process::exit_fd),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.native.as_ref().map_or(-1, Process::exit_fd),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let ms = timeout.min(Duration::from_millis(200)).as_millis() as i32;
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
        if r < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return fail("poll failed");
        }
        // Contact is processed first: simultaneous finger-up invalidates a sample.
        for (index, fd) in fds.iter().enumerate() {
            if index > 2 || fd.fd < 0 || fd.revents == 0 {
                continue;
            }
            let mut b = [0u8; 1024];
            let n = unsafe { libc::read(fd.fd, b.as_mut_ptr().cast(), b.len()) };
            if n < 0 {
                return fail("child read failed");
            }
            let b = &b[..n as usize];
            if index == 1 {
                require(!b.is_empty(), "service control disconnected")?;
                self.control.extend_from_slice(b);
                require(self.control.len() <= 128, "oversized control")?;
                while let Some(pos) = self.control.iter().position(|v| *v == b'\n') {
                    let bytes: Vec<_> = self.control.drain(..=pos).collect();
                    let line = std::str::from_utf8(&bytes[..pos]).map_err(|_| "invalid control")?;
                    self.control(line)?;
                }
                continue;
            }
            let proc = if index == 0 {
                self.contact.as_mut().unwrap()
            } else {
                self.native.as_mut().unwrap()
            };
            proc.bytes += b.len();
            require(proc.bytes <= 16384, "excessive child output")?;
            if b.is_empty() {
                proc.eof = true;
                require(proc.buffer.is_empty(), "truncated child record")?;
                continue;
            }
            proc.buffer.extend_from_slice(b);
            require(proc.buffer.len() <= 2048, "oversized child record")?;
            while let Some(pos) = proc.buffer.iter().position(|v| *v == b'\n') {
                require(pos <= 80, "oversized protocol line")?;
                let bytes: Vec<_> = proc.buffer.drain(..=pos).collect();
                let line =
                    std::str::from_utf8(&bytes[..pos]).map_err(|_| "invalid child record")?;
                if index == 0 {
                    self.c.record(line)?;
                } else {
                    self.n.record(line)?;
                }
            }
        }
        require(!STOP.load(Ordering::Relaxed), "cancelled")?;
        if let Some(p) = self.contact.as_mut() {
            if p.joined()? {
                self.c.complete(p.status.unwrap())?;
            }
        }
        if let Some(p) = self.native.as_mut() {
            if p.joined()? {
                self.n.complete(p.status.unwrap())?;
            }
        }
        Ok(())
    }
    fn wait(&mut self, secs: u64, predicate: impl Fn(&Self) -> bool) -> Result<()> {
        let end = Instant::now() + Duration::from_secs(secs);
        while !predicate(self) {
            let now = Instant::now();
            require(now < end, "operation timeout")?;
            self.pump(end - now)?;
        }
        Ok(())
    }
    fn joined_native(&self) -> bool {
        self.native
            .as_ref()
            .is_some_and(|p| p.eof && p.status.is_some())
    }
    fn joined_contact(&self) -> bool {
        self.contact
            .as_ref()
            .is_some_and(|p| p.eof && p.status.is_some())
    }
    fn run(&mut self) -> Result<()> {
        let op_start = Instant::now();
        self.start_native()?;
        if !matches!(self.operation.as_str(), "enroll" | "verify") {
            self.wait(30, |s| s.joined_native())?;
            let (code, id) = self
                .n
                .complete(self.native.as_ref().unwrap().status.unwrap())?;
            for id in &self.n.ids {
                emit(&format!("item {id}"))?;
            }
            return emit(&format!("result {code} {id}"));
        }
        if self.operation == "enroll" {
            self.wait(20, |s| s.n.state == NState::Prepared)?;
        }
        let mut stages = 0u32;
        for _ in 0..100 {
            self.start_contact()?;
            if self.operation == "verify" {
                self.wait(20, |s| s.n.state == NState::Standby)?;
                // Initialized and asleep until the touch.
                emit("power 2")?;
            }
            self.wait(
                if self.operation == "enroll" {
                    120
                } else {
                    86400
                },
                |s| s.c.state == CState::Down,
            )?;
            let touch = Instant::now();
            self.touch_at = Some(touch);
            if self.operation == "verify" {
                emit("power 1")?;
                self.n.state = NState::Activating;
                self.native.as_mut().unwrap().send("activate")?;
            }
            self.c.state = CState::Presenting;
            self.contact.as_mut().unwrap().send("present")?;
            self.wait(15, |s| {
                s.c.state == CState::Ready && s.n.state == NState::Prepared
            })?;
            let ready = Instant::now();
            self.n.state = NState::Capturing;
            self.n.captured = false;
            self.native.as_mut().unwrap().send("capture")?;
            self.wait(25, |s| s.n.captured || s.n.state == NState::Sample)?;
            let captured = Instant::now();
            // The finger may lift once the image is taken; the comparison
            // goes on without it.
            if self.n.captured {
                self.release_contact()?;
            }
            self.wait(25, |s| s.n.state == NState::Sample)?;
            let sample_at = Instant::now();
            let sample = self.n.sample.unwrap();
            if self.c.state != CState::Finishing {
                self.release_contact()?;
            }
            let verify = self.operation == "verify";
            // A verification's answer is the sample's: report it now, and
            // clean up the contact and the secure session after it, together.
            if verify {
                emit(&format!("result {} {}", sample.code, sample.id))?;
            }
            let reported = Instant::now();
            // Without a match the next attempt follows: the worker rests
            // with the sensor up instead of shutting down.
            let rest = verify && sample.code != 0;
            self.n.state = NState::Finishing;
            self.n.rest = rest;
            self.native
                .as_mut()
                .unwrap()
                .send(if rest { "rest" } else { "finish" })?;
            self.wait(5, |s| s.joined_contact())?;
            self.c
                .complete(self.contact.as_ref().unwrap().status.unwrap())?;
            let final_sample = verify || (sample.code == 0 && sample.remaining == 0);
            if final_sample {
                let mut result = None;
                if rest {
                    self.wait(5, |s| s.n.state == NState::Resting)?;
                    // It outlives this bridge.
                    self.native = None;
                } else {
                    self.wait(20, |s| s.joined_native())?;
                    result = Some(
                        self.n
                            .complete(self.native.as_ref().unwrap().status.unwrap())?,
                    );
                }
                let done = Instant::now();
                eprintln!("goodix timing phase_ms start_to_touch={} touch_to_ready={} capture={} compare={} touch_to_result={} cleanup_after_result={} rested={rest}",touch.duration_since(op_start).as_millis(),ready.duration_since(touch).as_millis(),captured.duration_since(ready).as_millis(),sample_at.duration_since(captured).as_millis(),reported.duration_since(touch).as_millis(),done.duration_since(reported).as_millis());
                if verify {
                    return emit(if rest { "power 2" } else { "power 0" });
                }
                let (code, id) = result.unwrap();
                emit("progress 20")?;
                return emit(&format!("result {code} {id}"));
            }
            self.wait(10, |s| s.n.state == NState::Prepared)?;
            if sample.code == 3 {
                emit("retry")?;
            }
            if sample.code == 0 {
                let progress =
                    (20 * sample.total.saturating_sub(sample.remaining) / sample.total).min(19);
                if progress > stages {
                    stages = progress;
                    emit(&format!("progress {progress}"))?;
                }
            }
        }
        fail("contact limit exceeded")
    }
    /// A verification that failed on its contact, a finger lifted before
    /// the image, leaves the worker warm like one without a match. Whatever
    /// the worker says before it rests belongs to the failed attempt.
    fn rest_after_failure(&mut self) {
        if self.operation != "verify"
            || STOP.load(Ordering::Relaxed)
            || !matches!(
                self.n.state,
                NState::Activating | NState::Prepared | NState::Capturing
            )
        {
            return;
        }
        let Some(native) = self.native.as_mut() else {
            return;
        };
        if native.eof || native.send("rest").is_err() {
            return;
        }
        let end = Instant::now() + Duration::from_secs(5);
        let mut said = std::mem::take(&mut native.buffer);
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            if !readable(native.output.as_raw_fd(), left.as_millis().min(200) as i32) {
                continue;
            }
            let mut b = [0u8; 1024];
            match native.output.read(&mut b) {
                Ok(n) if n > 0 && said.len() + n <= 4096 => said.extend_from_slice(&b[..n]),
                _ => return,
            }
            if said
                .split(|b| *b == b'\n')
                .any(|line| line == b"UFW1 resting")
            {
                self.native = None;
                return;
            }
        }
    }
    fn release_contact(&mut self) -> Result<()> {
        self.c.state = CState::Finishing;
        self.c.released = true;
        // A finger that lifted first has ended the child already; its
        // record decides, so a closed control is not an error here.
        let _ = self.contact.as_mut().unwrap().send("finish");
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        let mut bad = false;
        if let Some(p) = self.contact.as_mut() {
            bad |= p.stop().is_err();
        }
        if let Some(p) = self.native.as_mut() {
            bad |= p.stop().is_err();
        }
        require(!bad, "child cleanup unconfirmed")
    }
}
fn main_inner() -> Result<()> {
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let args: Vec<_> = env::args().collect();
    require(args.len() >= 2 && args.len() <= 3, "invalid arguments")?;
    let op = &args[1];
    require(
        matches!(
            op.as_str(),
            "probe" | "open" | "list" | "delete" | "enroll" | "verify"
        ),
        "unsupported operation",
    )?;
    let argument = if op == "delete" {
        require(
            args.len() == 3 && number(&args[2])? > 0,
            "invalid deletion id",
        )?;
        Some(args[2].clone())
    } else {
        require(args.len() == 2, "unexpected argument")?;
        None
    };
    let config = configuration()?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open("/run/lock/nx809j-fingerprint.lock")
        .map_err(|_| "lock unavailable")?;
    require(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "fingerprint busy",
    )?;
    let mut bridge = Bridge::new(config, op, argument);
    let result = bridge.run();
    if result.is_err() {
        if let Some(touch) = bridge.touch_at {
            eprintln!(
                "goodix timing failed phase={:?} touch_elapsed_ms={}",
                bridge.n.state,
                touch.elapsed().as_millis()
            );
        }
        bridge.rest_after_failure();
    }
    let cleanup = bridge.close();
    if let Err(error) = &cleanup {
        eprintln!("Goodix bridge cleanup: {error}");
    }
    result.and(cleanup)
}
fn main() {
    if let Err(error) = main_inner() {
        eprintln!("Goodix bridge: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contact_requires_clean_withdrawal() {
        let mut c = Contact::new();
        assert!(c.record("DFP1 ready 1").is_err());
        c.record("DFP1 waiting").unwrap();
        c.record("DFP1 down 1").unwrap();
        c.state = CState::Presenting;
        c.record("DFP1 ready 1").unwrap();
        assert!(c.record("DFP1 lost 1").is_err());
        c.state = CState::Finishing;
        assert!(c.complete(true).is_err());
        c.record("DFP1 stopped 1 0").unwrap();
        assert!(c.complete(false).is_err());
        c.complete(true).unwrap();
    }
    fn sample() -> Native {
        let mut n = Native::new("verify");
        n.record("UFW1 standby").unwrap();
        assert!(n.record("UFW1 prepared").is_err());
        n.state = NState::Activating;
        n.record("UFW1 prepared").unwrap();
        n.state = NState::Capturing;
        n.record("UFW1 sample 0 100 123 0").unwrap();
        n
    }
    #[test]
    fn result_requires_matching_sample_and_confirmed_cleanup() {
        let mut n = sample();
        assert!(n.record("UFF1 result 0 123").is_err());
        n.state = NState::Finishing;
        assert!(n.record("UFF1 result 0 123").is_err());
        n.record("UFW1 cleaned").unwrap();
        assert!(n.record("UFF1 result 0 124").is_err());
        n.record("UFF1 result 0 123").unwrap();
        assert!(n.complete(false).is_err());
        assert_eq!(n.complete(true).unwrap(), (0, 123));
    }
    #[test]
    fn verify_rejects_false_identifiers_and_list_duplicates() {
        let mut n = Native::new("verify");
        n.state = NState::Capturing;
        assert!(n.record("UFW1 sample 1 0 123 0").is_err());
        let mut n = Native::new("list");
        n.record("UFF1 item 1").unwrap();
        assert!(n.record("UFF1 item 1").is_err());
    }
    #[test]
    fn enrollment_retry_keeps_session_but_cannot_finish_early() {
        let mut n = Native::new("enroll");
        n.record("UFW1 prepared").unwrap();
        n.state = NState::Capturing;
        n.record("UFW1 sample 3 100 0 0").unwrap();
        n.state = NState::Finishing;
        n.record("UFW1 idle").unwrap();
        n.state = NState::Capturing;
        n.record("UFW1 sample 0 2 123 3").unwrap();
        n.state = NState::Finishing;
        n.record("UFW1 cleaned").unwrap();
        assert!(n.record("UFF1 result 0 123").is_err());
    }
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("goodix-bridge-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    /// Runs a verification against two scripted children, or a warm
    /// worker listening at `warm`. `@` in a script is its scratch directory.
    fn verify_with(name: &str, worker: &str, contact: &str) -> Result<()> {
        let result = verify_at(name, worker, contact, None);
        std::fs::remove_dir_all(scratch(name)).unwrap();
        result
    }
    fn verify_at(name: &str, worker: &str, contact: &str, warm: Option<&Path>) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        // A script written while another test forks is busy text for exec.
        static SCRIPTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serial = SCRIPTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch(name);
        let worker = worker.replace('@', dir.to_str().unwrap());
        let (script, worker) = (worker.as_str(), dir.join("worker"));
        std::fs::write(&worker, script).unwrap();
        let (script, contact) = (contact, dir.join("contact"));
        std::fs::write(&contact, script).unwrap();
        for path in [&worker, &contact] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let mut bridge = Bridge::new(
            Config {
                worker,
                firmware: dir.join("firmware"),
                contact,
            },
            "verify",
            None,
        );
        bridge.control_fd = fds[0];
        bridge.warm = warm.map_or_else(|| dir.join("warm.sock"), Path::to_path_buf);
        let result = bridge.run();
        if result.is_err() {
            bridge.rest_after_failure();
        }
        let cleanup = bridge.close();
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        result.and(cleanup)
    }

    const WORKER: &str = "#!/bin/sh\necho 'UFW1 standby'\nread c; [ \"$c\" = activate ] || exit 2\necho 'UFW1 prepared'\nread c; [ \"$c\" = capture ] || exit 2\necho 'UFW1 captured'\necho 'UFW1 sample 0 100 123 0'\nread c; [ \"$c\" = finish ] || exit 2\necho 'UFW1 cleaned'\necho 'UFF1 result 0 123'\n";
    const HOLD: &str = "#!/bin/sh\necho 'DFP1 waiting'\necho 'DFP1 down 1'\nread c; [ \"$c\" = present ] || exit 2\necho 'DFP1 ready 1'\nread c; [ \"$c\" = finish ] || exit 2\necho 'DFP1 stopped 1 0'\n";

    #[test]
    fn bridge_runs_both_children_and_requires_cleanup() {
        verify_with("hold", WORKER, HOLD).unwrap();
        // A worker that never says it captured still finishes at the sample.
        verify_with("plain", &WORKER.replace("echo 'UFW1 captured'\n", ""), HOLD).unwrap();
    }

    #[test]
    fn a_lift_after_the_capture_is_clean_and_before_it_fails() {
        let lift = "#!/bin/sh\necho 'DFP1 waiting'\necho 'DFP1 down 1'\nread c; [ \"$c\" = present ] || exit 2\necho 'DFP1 ready 1'\nread c; [ \"$c\" = finish ] || exit 2\necho 'DFP1 lost 1'\nexit 2\n";
        verify_with("lift", WORKER, lift).unwrap();
        let early = "#!/bin/sh\necho 'DFP1 waiting'\necho 'DFP1 down 1'\nread c; [ \"$c\" = present ] || exit 2\necho 'DFP1 ready 1'\necho 'DFP1 lost 1'\nexit 2\n";
        let slow = WORKER.replace(
            "read c; [ \"$c\" = capture ] || exit 2\n",
            "read c; [ \"$c\" = capture ] || exit 2\nsleep 0.3\n",
        );
        assert!(verify_with("early", &slow, early).is_err());
    }

    #[test]
    fn without_a_match_the_worker_rests_and_the_bridge_leaves_it() {
        // It stays up after resting; the bridge must not wait for it.
        let rest = "#!/bin/sh\necho 'UFW1 standby'\nread c; [ \"$c\" = activate ] || exit 2\necho 'UFW1 prepared'\nread c; [ \"$c\" = capture ] || exit 2\necho 'UFW1 captured'\necho 'UFW1 sample 1 100 0 0'\nread c; [ \"$c\" = rest ] || exit 2\necho 'UFW1 resting'\nsleep 3\n";
        let start = Instant::now();
        verify_with("rest", rest, HOLD).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        // A finger lifted before the image fails the attempt, and rests too.
        let early = "#!/bin/sh\necho 'DFP1 waiting'\necho 'DFP1 down 1'\nread c; [ \"$c\" = present ] || exit 2\necho 'DFP1 ready 1'\necho 'DFP1 lost 1'\nexit 2\n";
        let lifted = "#!/bin/sh\necho 'UFW1 standby'\nread c; [ \"$c\" = activate ] || exit 2\necho 'UFW1 prepared'\nsleep 0.3\nread c; [ \"$c\" = capture ] && read c; [ \"$c\" = rest ] || exit 2\ntouch @/rested\necho 'UFW1 resting'\nsleep 3\n";
        assert!(verify_at("lifted", lifted, early, None).is_err());
        assert!(scratch("lifted").join("rested").exists());
        std::fs::remove_dir_all(scratch("lifted")).unwrap();
    }

    #[test]
    fn a_warm_worker_is_adopted_instead_of_starting_one() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;
        let dir = scratch("warm-socket");
        let path = dir.join("verify.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let warm = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            let mut lines = BufReader::new(stream).lines();
            let mut say = |line: &str| writeln!(out, "{line}").unwrap();
            say("UFW1 standby");
            assert_eq!(lines.next().unwrap().unwrap(), "activate");
            say("UFW1 prepared");
            assert_eq!(lines.next().unwrap().unwrap(), "capture");
            say("UFW1 captured");
            say("UFW1 sample 0 100 123 0");
            assert_eq!(lines.next().unwrap().unwrap(), "finish");
            say("UFW1 cleaned");
            say("UFF1 result 0 123");
        });
        // The worker on disk would fail: only the warm one can succeed.
        verify_at("adopt", "#!/bin/sh\nexit 2\n", HOLD, Some(&path)).unwrap();
        warm.join().unwrap();
        std::fs::remove_dir_all(scratch("adopt")).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
