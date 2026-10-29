//! ledger: what ledgerd booked.

use std::path::Path;
use std::process::ExitCode;

use ledger::book::{self, RUN_DIR, STATE_DIR, hour_of};
use ledger::report;
use ledger::sample::{Power, Sample, Screen, clock};

const USAGE: &str = "usage: ledger [summary|hours|builds] [--last 24h|7d|90m]
       ledger now";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ledger: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `24h`, `7d` or `90m` in seconds.
fn span(text: &str) -> Option<i64> {
    let (n, unit) = text.split_at(text.len().checked_sub(1)?);
    let n: i64 = n.parse().ok()?;
    let unit = match unit {
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return None,
    };
    Some(n * unit)
}

fn run(args: Vec<String>) -> Result<String, String> {
    let mut command = "summary";
    let mut last = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--last" => {
                let value = it.next().ok_or(USAGE)?;
                last = Some(span(value).ok_or(USAGE)?);
            }
            "summary" | "hours" | "builds" | "now" => command = arg,
            "-h" | "--help" => return Ok(format!("{USAGE}\n")),
            _ => return Err(USAGE.into()),
        }
    }
    if command == "now" {
        return now();
    }
    let last = last.unwrap_or(if command == "builds" {
        31 * 86400
    } else {
        86400
    });
    let current = hour_of(clock(libc::CLOCK_REALTIME));
    let since = current - (last - 1).max(0) / 3600 * 3600;
    let mut buckets = book::load(Path::new(STATE_DIR));
    buckets.extend(book::load_open(Path::new(RUN_DIR)));
    buckets.retain(|b| b.hour >= since);
    let title = format!(
        "{}, last {} from {}",
        ledger::sysfs::read("/proc/sys/kernel/hostname").unwrap_or_default(),
        report::duration(last as f64),
        report::local(since, "%a %d %H:%M")
    );
    Ok(match command {
        "hours" => report::hours(&buckets),
        "builds" => report::builds(&buckets),
        _ => report::summary(&buckets, &title),
    })
}

fn now() -> Result<String, String> {
    let power = Power::find().ok_or("no battery here")?;
    let s = Sample::read(&power, &Screen::find());
    let mut text = format!("{}  {:.3} W", s.state().label(), s.watts);
    if let Some(level) = s.level {
        text.push_str(&format!("  {level:.0}%"));
    }
    if let (true, Some(b)) = (s.screen, s.brightness) {
        text.push_str(&format!("  brightness {:.0}%", b * 100.0));
    }
    text.push('\n');
    Ok(text)
}
