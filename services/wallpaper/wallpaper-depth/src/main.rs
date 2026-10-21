//! Lock-screen depth masks on the GPU: BiRefNet_lite and Depth Anything V2
//! Small run as native Vulkan compute through wgpu.

// Kernel builders take their shapes and tiling as plain arguments.
#![allow(clippy::too_many_arguments)]

mod bench;
mod fuse;
mod gpu;
mod kernels;
mod onnx;
mod pipeline;
mod plan;
mod reference;
mod tensor;
mod tex;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

fn main() -> Result<()> {
    run_in_background();
    pin_to_fast_cores();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("check") if args.len() >= 5 => check(&args[2], &args[3], &args[4], args.get(5).map(String::as_str)),
        Some("graph") if args.len() >= 4 => graph(&args[2], &args[3]),
        Some("profile") if args.len() >= 4 => profile(&args[2], &args[3]),
        Some("mask") if args.len() >= 3 => mask(&args[2]),
        Some("half") if args.len() >= 4 => half(&args[2], &args[3]),
        Some("bench-gemm") if args.len() >= 5 => {
            let dim = |i: usize| args[i].parse::<usize>().context("dimension");
            bench::gemm(&gpu::Gpu::new()?, dim(2)?, dim(3)?, dim(4)?, args.get(5).map(String::as_str))
        }
        _ => bail!(
            "usage: wallpaper-depth mask WALLPAPER\n       wallpaper-depth half MODEL.onnx OUT.onnx\n       wallpaper-depth check MODEL.onnx INPUT.npy REFERENCE.npy [nodes|nodes=OP]"
        ),
    }
}

/// Runs as an ordinary low-priority job even when started from a real-time
/// thread, as Denial's shell does: the policy carries across fork. Threads
/// started later inherit both settings.
fn run_in_background() {
    // SAFETY: plain syscalls that only lower this thread's own priority.
    unsafe {
        let normal = libc::sched_param { sched_priority: 0 };
        libc::sched_setscheduler(0, libc::SCHED_OTHER, &normal);
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// Keeps the process off the slowest cluster: on the phones the little
/// cores take several times as long for the same work. Threads started
/// later inherit the mask.
fn pin_to_fast_cores() {
    let freq = |c: usize| {
        std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq")).ok()?.trim().parse::<u64>().ok()
    };
    let cpus: Vec<(usize, u64)> = (0..libc::CPU_SETSIZE as usize).map_while(|c| freq(c).map(|f| (c, f))).collect();
    let Some(slowest) = cpus.iter().map(|c| c.1).min() else { return };
    if cpus.iter().all(|c| c.1 == slowest) {
        return;
    }
    // SAFETY: the set is a plain bitmask, zeroed before use.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &(c, f) in &cpus {
            if f > slowest {
                libc::CPU_SET(c, &mut set);
            }
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

/// Loads a model and applies the graph rewrites, unless
/// WALLPAPER_DEPTH_NO_FUSE is set.
fn load(path: &str) -> Result<onnx::Model> {
    let mut model = onnx::load(Path::new(path))?;
    if std::env::var_os("WALLPAPER_DEPTH_NO_FUSE").is_none() {
        let before = model.nodes.len();
        let n = fuse::deform_convs(&mut model)?;
        if n > 0 {
            eprintln!("fused {n} deformable convolutions, {before} -> {} nodes", model.nodes.len());
        }
    }
    Ok(model)
}

/// Prints the GPU steps of a model with operand shapes; constants are marked.
fn graph(model: &str, input: &str) -> Result<()> {
    let model = load(model)?;
    let x = tensor::read_npy(Path::new(input))?;
    let program = plan::plan(model, &x.shape, &gpu::Gpu::new()?.options()?)?;
    let describe = |v: &plan::Val| match v {
        plan::Val::Const(t) => format!("C{:?}", t.shape),
        plan::Val::Gpu(v) => format!("{:?}{}", v.shape, if v.is_contiguous() { "" } else { "~" }),
        plan::Val::Lazy(l) => format!("{:?}λ{}", l.shape, l.loads.len()),
        plan::Val::Rows(r) => format!("{:?}ρ{}", r.shape, r.x.loads.len()),
        plan::Val::Cat(c) => format!("{:?}‖{}", c.shape, c.parts.len()),
    };
    for (i, s) in program.steps.iter().enumerate() {
        let node = &program.nodes[s.node];
        let ins: Vec<String> = s.inputs.iter().map(|v| v.as_ref().map_or("-".into(), describe)).collect();
        let outs: Vec<String> = s.outputs.iter().map(describe).collect();
        println!("{i} {} {} [{}] -> {} ({} dispatches)", node.op, node.name, ins.join(", "), outs.join(", "), s.dispatches.len());
    }
    Ok(())
}

/// Times every dispatch on its own; prints the slowest and totals per op.
fn profile(model: &str, input: &str) -> Result<()> {
    let model = load(model)?;
    let x = tensor::read_npy(Path::new(input))?;
    let gpu = gpu::Gpu::new()?;
    let program = plan::plan(model, &x.shape, &gpu.options()?)?;
    let runner = gpu::Runner::new(&gpu, &program)?;
    if std::env::var_os("WALLPAPER_DEPTH_TRACE").is_none() {
        runner.run(&x)?;
    }
    let times = runner.profile(&x)?;
    let total: f64 = times.iter().map(|t| t.1).sum();
    // Per op and kernel kind, then per node; GEMM rates in GMAC/s.
    let mut by_op: std::collections::BTreeMap<String, (f64, usize, u64)> = Default::default();
    let mut by_node: std::collections::HashMap<&str, (f64, usize, u64)> = Default::default();
    for ((label, s), k) in times.iter().zip(&program.kernels) {
        let kind = label.rfind(" [").map_or("", |i| &label[i..]);
        let e = by_op.entry(format!("{}{kind}", label.split(' ').next().unwrap_or(""))).or_default();
        let gemm = kind == " [gemm]" || k.split.is_some() || k.work != 0 && kind.is_empty() && label.starts_with("MatMul");
        let work = if gemm { k.work } else { 0 };
        e.0 += s;
        e.1 += 1;
        e.2 += work;
        let n = by_node.entry(label.as_str()).or_default();
        n.0 += s;
        n.1 += 1;
        n.2 += work;
    }
    let rate = |s: f64, w: u64| if w > 0 { format!("{:>6.0} GMAC/s", w as f64 / s / 1e9) } else { String::new() };
    let mut ops: Vec<_> = by_op.into_iter().collect();
    ops.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
    println!("{} dispatches, {:.1} ms in all", times.len(), total * 1e3);
    for (op, (s, n, w)) in ops {
        println!("{:>9.2} ms {:>5}x {op:<24} {}", s * 1e3, n, rate(s, w));
    }
    let mut slow: Vec<_> = by_node.into_iter().collect();
    slow.sort_by(|a, b| b.1.0.total_cmp(&a.1.0));
    let top = std::env::var("WALLPAPER_DEPTH_PROFILE_TOP").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    for (label, (s, n, w)) in slow.iter().take(top) {
        println!("{:>9.2} ms {:>4}x {} {label}", s * 1e3, n, rate(*s, *w));
    }
    Ok(())
}

/// Runs a model on a reference input and compares the output with ORT's.
fn check(model: &str, input: &str, reference: &str, nodes: Option<&str>) -> Result<()> {
    let started = Instant::now();
    let model = load(model)?;
    let x = tensor::read_npy(Path::new(input))?;
    let want = tensor::read_npy(Path::new(reference))?;
    let count = model.nodes.len();
    let gpu = gpu::Gpu::new()?;
    // Node checks compare every node's own output, so nothing stays lazy.
    let opts = plan::Options { fuse: nodes.is_none(), ..gpu.options()? };
    let program = plan::plan(model, &x.shape, &opts)?;
    println!(
        "planned {count} nodes into {} GPU steps, {} dispatches, {} buffers in {:.2?}",
        program.steps.len(),
        program.kernels.len(),
        program.bufs.len(),
        started.elapsed()
    );
    let started = Instant::now();
    let runner = gpu::Runner::new(&gpu, &program)?;
    println!(
        "{}: {} pipelines, {} submissions, {:.1} MB weights, {:.1} MB scratch (peak live {:.1} MB), ready in {:.2?}",
        gpu.name,
        runner.pipeline_count(),
        runner.submissions(),
        runner.weight_bytes as f64 / 1e6,
        runner.scratch_bytes as f64 / 1e6,
        runner.peak_live as f64 / 1e6,
        started.elapsed()
    );
    if let Some(n) = nodes {
        runner.check_nodes(&x, n.strip_prefix("nodes=")).context("node check")?;
    }
    for run in 0..3 {
        let (outs, elapsed) = runner.run(&x)?;
        let got = &outs[0];
        let got = if got.shape != want.shape && got.numel() == want.numel() { got.reshaped(want.shape.clone()) } else { got.clone() };
        let (err, scale) = gpu::compare(&got, &want)?;
        println!("run {run}: {elapsed:.2?}, max err {err:.3e} against max |ref| {scale:.3e}");
    }
    Ok(())
}

/// Where the models live: WALLPAPER_DEPTH_MODELS, else
/// `share/wallpaper-depth/models` in the prefix the executable is installed
/// in.
fn models_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("WALLPAPER_DEPTH_MODELS") {
        return Ok(dir.into());
    }
    let exe = std::env::current_exe()?;
    let prefix = exe.parent().and_then(Path::parent).context("executable has no prefix")?;
    Ok(prefix.join("share/wallpaper-depth/models"))
}

/// Writes a copy of a model with its weights in float16.
fn half(input: &str, output: &str) -> Result<()> {
    let model = std::fs::read(input).with_context(|| format!("reading {input}"))?;
    let halved = onnx::halve(&model)?;
    let tmp = format!("{output}.tmp");
    std::fs::write(&tmp, &halved)?;
    std::fs::rename(&tmp, output)?;
    println!("{output}: {} MB, from {} MB", halved.len() >> 20, model.len() >> 20);
    Ok(())
}

/// Plans a model for one input shape, then runs it on the input that
/// arrives on `input`. Returns its first output, the planning time and the
/// GPU time.
fn infer(
    gpu: &gpu::Gpu,
    model: &Path,
    shape: &[usize],
    input: std::sync::mpsc::Receiver<tensor::Tensor>,
) -> Result<(tensor::Tensor, Duration, Duration)> {
    let started = Instant::now();
    let model = load(model.to_str().context("model path")?)?;
    let loaded = started.elapsed();
    let program = plan::plan(model, shape, &gpu.options()?)?;
    let planned = started.elapsed();
    let runner = gpu::Runner::new(gpu, &program)?;
    let ready = started.elapsed();
    if std::env::var_os("WALLPAPER_DEPTH_VERBOSE").is_some() {
        eprintln!("load {loaded:.2?}, plan {:.2?}, runner {:.2?}", planned - loaded, ready - planned);
    }
    let input = input.recv().context("no model input")?;
    let (mut outs, elapsed) = runner.run(&input)?;
    Ok((outs.swap_remove(0), ready, elapsed))
}

/// The lock mask for one wallpaper, written with its subject and depth to
/// $XDG_CACHE_HOME/denial/wallpaper-depth, where the Denial lock screen
/// reads it.
fn mask(wallpaper: &str) -> Result<()> {
    let started = Instant::now();
    let models = models_dir()?;
    let cache = match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(&std::env::var_os("HOME").context("HOME is not set")?).join(".cache"),
    };
    let out = cache.join("denial/wallpaper-depth");
    let (w, h) = image::image_dimensions(wallpaper).with_context(|| format!("reading {wallpaper}"))?;
    let (w, h) = (w as usize, h as usize);

    // Each model plans on its own thread and runs once its input is ready;
    // both plan while the wallpaper decodes, and Depth Anything runs while
    // BiRefNet is still planning.
    let gpu = gpu::Gpu::new()?;
    let (subject_tx, subject_rx) = std::sync::mpsc::channel();
    let (depth_tx, depth_rx) = std::sync::mpsc::channel();
    let (dw, dh) = pipeline::depth_size(w, h);
    let (gpu_ref, models_ref) = (&gpu, &models);
    let (decode, prepare, subject, depth) = std::thread::scope(|s| -> Result<_> {
        // Owned here, so a failed decode hangs up on the model threads.
        let (subject_tx, depth_tx) = (subject_tx, depth_tx);
        let subject = s.spawn(move || infer(gpu_ref, &models_ref.join("birefnet_lite.onnx"), &[1, 3, 1024, 1024], subject_rx));
        let depth = s.spawn(move || infer(gpu_ref, &models_ref.join("depth_anything_v2_small.onnx"), &[1, 3, dh, dw], depth_rx));
        let image = image::open(wallpaper).with_context(|| format!("reading {wallpaper}"))?.into_rgb8();
        ensure!((image.width() as usize, image.height() as usize) == (w, h), "{wallpaper} changed while reading it");
        let rgb = image.as_raw();
        let decode = started.elapsed();
        let t = Instant::now();
        let depth_in = pipeline::model_input(rgb, w, h, dw, dh, pipeline::Filter::Bicubic);
        let subject_in = pipeline::model_input(rgb, w, h, 1024, 1024, pipeline::Filter::Bilinear);
        let prepare = t.elapsed();
        if let Some(dir) = std::env::var_os("WALLPAPER_DEPTH_DUMP") {
            let dir = Path::new(&dir);
            std::fs::write(dir.join("rgb.raw"), rgb)?;
            tensor::write_npy(&dir.join("subject_in.npy"), &subject_in)?;
            tensor::write_npy(&dir.join("depth_in.npy"), &depth_in)?;
        }
        let _ = depth_tx.send(depth_in);
        let _ = subject_tx.send(subject_in);
        let subject = subject.join().expect("BiRefNet thread")?;
        let depth = depth.join().expect("Depth Anything thread")?;
        Ok((decode, prepare, subject, depth))
    })?;
    let (logits, subject_plan, subject_gpu) = subject;
    let (depth, depth_plan, depth_gpu) = depth;
    if let Some(dir) = std::env::var_os("WALLPAPER_DEPTH_DUMP") {
        tensor::write_npy(&Path::new(&dir).join("logits.npy"), &logits)?;
    }

    let t = Instant::now();
    let mask = pipeline::lock_mask(&logits, &depth, pipeline::mask_size(w, h))?;
    let compose = t.elapsed();
    let t = Instant::now();
    std::fs::create_dir_all(&out)?;
    let (mw, mh) = (mask.width as u32, mask.height as u32);
    let rgba: Vec<u8> = mask.alpha.iter().flat_map(|&a| [255, 255, 255, a]).collect();
    write_png(&out.join("subject.png"), mw, mh, &mask.subject, image::ExtendedColorType::L8)?;
    let n = depth.shape.len();
    tensor::write_npy(&out.join("depth.npy"), &depth.reshaped(depth.shape[n - 2..].to_vec()))?;
    write_png(&out.join("lock-mask.png"), mw, mh, &rgba, image::ExtendedColorType::Rgba8)?;
    let write = t.elapsed();

    let ms = |d: Duration| (d.as_secs_f64() * 1e3).round();
    let sidecar = format!(
        concat!(
            r#"{{"wallpaper": {:?}, "variant": "native-gpu", "gpu": {:?}, "subject_model": "birefnet_lite.onnx", "#,
            r#""depth_gate": {{"low": {}, "high": {}, "grow": {}}}, "decode_ms": {}, "prepare_ms": {}, "#,
            r#""subject_plan_ms": {}, "subject_gpu_ms": {}, "depth_plan_ms": {}, "depth_gpu_ms": {}, "#,
            r#""compose_ms": {}, "write_ms": {}, "total_ms": {}}}"#
        ),
        wallpaper,
        gpu.name,
        pipeline::LOW,
        pipeline::HIGH,
        pipeline::GROW,
        ms(decode),
        ms(prepare),
        ms(subject_plan),
        ms(subject_gpu),
        ms(depth_plan),
        ms(depth_gpu),
        ms(compose),
        ms(write),
        ms(started.elapsed()),
    );
    let tmp = out.join("lock-mask.json.tmp");
    std::fs::write(&tmp, &sidecar)?;
    std::fs::rename(&tmp, out.join("lock-mask.json"))?;
    println!("{sidecar}");
    Ok(())
}

/// Writes a PNG through a temporary file so readers never see half of it.
fn write_png(path: &Path, w: u32, h: u32, data: &[u8], color: image::ExtendedColorType) -> Result<()> {
    use image::ImageEncoder;
    let tmp = path.with_extension("png.tmp");
    let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    image::codecs::png::PngEncoder::new_with_quality(file, image::codecs::png::CompressionType::Fast, Default::default())
        .write_image(data, w, h, color)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
