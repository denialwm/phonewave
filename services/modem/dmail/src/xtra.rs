//! Predicted orbits (XTRA) for the GNSS engine: Qualcomm's file, as stock
//! Android's xtra-daemon fetches it, kept in dmail's state directory. On the
//! 12R, with all other assistance data wiped, it made the first fix 191 s at
//! ±10 m, where stale data took 4.7 min at ±535 m.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// GPS, GLONASS, BeiDou, Galileo and QZSS, from the stock xtra-daemon.
const URLS: [&str; 3] = [
    "https://path1.xtracloud.net/xtra3grcej.bin",
    "https://path2.xtracloud.net/xtra3grcej.bin",
    "https://path3.xtracloud.net/xtra3grcej.bin",
];
const FILE: &str = "xtra.bin";
/// A file this old is fetched again. Qualcomm's covers a week; fresher
/// predictions are closer.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
const DOWNLOAD_TIMEOUT: &str = "30";
const MAX_SIZE: &str = "1048576";

pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

/// The kept file, if it is younger than `MAX_AGE`.
pub fn fresh(state_dir: &Path) -> Option<Vec<u8>> {
    let path = path(state_dir);
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())?;
    if age > MAX_AGE {
        return None;
    }
    std::fs::read(path).ok().filter(|d| !d.is_empty())
}

/// Fetches the file with curl, from the first server that answers, and keeps
/// it. Blocks: run it off the core's thread.
pub fn download(state_dir: &Path) -> Result<Vec<u8>, String> {
    let partial = state_dir.join(format!("{FILE}.part"));
    let mut failures = Vec::new();
    for url in URLS {
        let out = Command::new("curl")
            .args(["--fail", "--silent", "--show-error", "--location"])
            .args(["--max-time", DOWNLOAD_TIMEOUT, "--max-filesize", MAX_SIZE])
            .arg("--output")
            .arg(&partial)
            .arg(url)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if out.status.success() {
            let data = std::fs::read(&partial).map_err(|e| format!("{FILE}: {e}"))?;
            if data.is_empty() {
                failures.push(format!("{url}: empty"));
                continue;
            }
            std::fs::rename(&partial, path(state_dir)).map_err(|e| format!("{FILE}: {e}"))?;
            return Ok(data);
        }
        let why = String::from_utf8_lossy(&out.stderr);
        failures.push(format!("{url}: {}", why.trim()));
    }
    let _ = std::fs::remove_file(&partial);
    Err(failures.join("; "))
}
