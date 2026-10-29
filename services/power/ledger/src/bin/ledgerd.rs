//! ledgerd: books where the battery goes, every ten seconds while the phone
//! is awake. It never wakes the phone: a sleep shows afterwards as a gap
//! between the monotonic and boot clocks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ledger::book::{self, Book, KEEP_DAYS, RUN_DIR, STATE_DIR, hour_of};
use ledger::counters::Hardware;
use ledger::sample::{Power, Sample, Screen};
use ledger::sysfs::read;

const TICK_MS: i32 = 10_000;
/// Paths whose link target or first line names a build, one per line.
const TAGS: &str = "/etc/ledger/tags";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ledgerd: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The kernel release and each tagged build.
fn build(tags: &[PathBuf]) -> String {
    let mut parts = vec![read("/proc/sys/kernel/osrelease").unwrap_or_default()];
    for tag in tags {
        let name = match fs::read_link(tag) {
            Ok(target) => target.file_name().map(|n| n.to_string_lossy().into_owned()),
            Err(_) => read(tag).and_then(|t| t.lines().next().map(str::to_owned)),
        };
        parts.extend(name.filter(|n| !n.is_empty()));
    }
    parts.join(" ")
}

/// A descriptor that reads SIGTERM and SIGINT, which are blocked.
fn signals() -> Result<libc::c_int, String> {
    // SAFETY: plain libc calls on a local, initialised signal set.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        if libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return Err("sigprocmask failed".into());
        }
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC);
        if fd < 0 {
            return Err("signalfd failed".into());
        }
        Ok(fd)
    }
}

/// Waits a tick; true if a signal came instead.
fn wait(fd: libc::c_int) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd. poll counts on the monotonic clock, so a
    // sleep of the phone does not use the tick up.
    let n = unsafe { libc::poll(&mut pfd, 1, TICK_MS) };
    n > 0 && pfd.revents & libc::POLLIN != 0
}

fn run() -> Result<(), String> {
    let Some(power) = Power::find() else {
        eprintln!("ledgerd: no battery here");
        return Ok(());
    };
    let screen = Screen::find();
    let hardware = Hardware::find();
    let tags: Vec<PathBuf> = read(TAGS)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect();
    let signals = signals()?;
    let state = Path::new(STATE_DIR);
    let run = Path::new(RUN_DIR);

    // Hours a crashed daemon left open in /run.
    let mut book = Book {
        open: book::load_open(run),
    };
    let mut before = Sample::read(&power, &screen);
    let mut counters = hardware.read();
    let mut trimmed_day = i64::MIN;
    eprintln!(
        "ledgerd: battery {}, booking {}",
        power.battery().display(),
        build(&tags)
    );

    while !wait(signals) {
        let now = Sample::read(&power, &screen);
        let next = hardware.read();
        let usage = hardware.usage(&counters, &next);
        book.account(&before, &now, usage, &build(&tags));
        before = now;
        counters = next;

        let closed = book.close_before(hour_of(before.real));
        if let Err(e) = book::append(state, &closed) {
            eprintln!("ledgerd: {}: {e}", state.display());
            // Keep them for the next try.
            book.open.extend(closed);
        }
        let day = hour_of(before.real) / 86400;
        if day != trimmed_day {
            trimmed_day = day;
            let oldest = hour_of(before.real) - KEEP_DAYS * 86400;
            if let Err(e) = book::trim(state, oldest) {
                eprintln!("ledgerd: trimming: {e}");
            }
        }
        if let Err(e) = book::save_open(run, &book.open) {
            eprintln!("ledgerd: {}: {e}", run.display());
        }
    }

    book::append(state, &book.open).map_err(|e| format!("{}: {e}", state.display()))?;
    book::remove_open(run);
    Ok(())
}
