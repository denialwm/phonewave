//! Start the exact Pad 4 factory ADSP from systemd after display initialization.
//! Firmware stays under an explicit runtime name so remoteproc's early probe
//! cannot bypass service ordering. This never stops/restarts a running DSP.
use std::{env, error::Error, fs, path::PathBuf, process::Command, thread, time::{Duration, Instant}};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const FIRMWARE: &str = "qcom/kaanapali/iceland/adsp-runtime.mbn";
const FILES: &[(&str, &str)] = &[
    ("/usr/lib/firmware/qcom/kaanapali/iceland/adsp-runtime.mbn", "eb56ce0322e1616d6eec0d482e0d63bd523066fdc0a29027bdf56be07902eeca"),
    ("/usr/lib/firmware/qcom/kaanapali/iceland/adsp_dtb.mbn", "09e35c4294fd3b3d25146603538c5e858af7e13a5d60a639f495783d397c448a"),
    ("/usr/lib/firmware/qcom/kaanapali/OnePlus-Pad4-tplg.bin", "11b78ff1037690499ecbc539d2ca19cde8ca0a86f62e6df00083fb88b0e967c7"),
];

fn verified_adsp() -> Result<PathBuf> {
    if fs::read_to_string("/etc/pad4-root-id")?.trim() != "59b8e7b3:archroot-v1" {
        return Err("not the authorized Pad 4 root filesystem".into());
    }
    let config = fs::read_to_string("/proc/bootconfig")?;
    if !config.lines().any(|l| l == "androidboot.serialno = \"59b8e7b3\"") {
        return Err("device serial mismatch".into());
    }
    for (file, expected) in FILES {
        let output = Command::new("/usr/bin/sha256sum").arg(file).output()?;
        if !output.status.success() || String::from_utf8(output.stdout)?.split_whitespace().next() != Some(expected) {
            return Err(format!("firmware/topology hash mismatch: {file}").into());
        }
    }
    let mut found = Vec::new();
    for entry in fs::read_dir("/sys/class/remoteproc")? {
        let path = entry?.path();
        if fs::read_to_string(path.join("name"))?.trim() == "adsp" { found.push(path); }
    }
    if found.len() != 1 { return Err("expected exactly one audio DSP".into()); }
    Ok(found.remove(0))
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 1 || !matches!(args[0].as_str(), "--check" | "--start") {
        return Err("usage: pad4-audio-init --check|--start".into());
    }
    let adsp = verified_adsp()?;
    let state = fs::read_to_string(adsp.join("state"))?;
    println!("Verified factory ADSP and topology; state={}", state.trim());
    if args[0] == "--check" { return Ok(()); }
    if !matches!(state.trim(), "offline" | "running") {
        return Err(format!("refusing to change ADSP in state {}", state.trim()).into());
    }
    // Load only modules from the running kernel's normal, audited module tree.
    for module in ["snd-q6apm", "q6apm-dai", "q6apm-lpass-dais", "q6prm-clocks", "snd-soc-sc8280xp"] {
        if !Command::new("/usr/bin/modprobe").arg(module).status()?.success() {
            return Err(format!("cannot load {module}").into());
        }
    }
    if state.trim() == "offline" {
        fs::write(adsp.join("firmware"), FIRMWARE)?;
        println!("Starting ADSP with verified runtime firmware");
        fs::write(adsp.join("state"), "start")?;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if fs::read_to_string("/proc/asound/cards")?.contains("OnePlus-Pad4") {
            println!("OnePlus Pad 4 sound card registered");
            return Ok(());
        }
        if Instant::now() >= deadline { return Err("ADSP started but sound card did not register; inspect journal".into()); }
        thread::sleep(Duration::from_millis(250));
    }
}
