//! Runs a planned program on a Vulkan device through wgpu. Scratch buffers
//! share physical memory once dead, and GPU objects are made as the first
//! run reaches them.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::kernels::{self, Kernel, View};
use crate::plan::{Buf, Program, Tex, Val};
use crate::reference;
use crate::tensor::{DType, Tensor, numel, offsets};
use crate::tex::{TexDesc, TexFormat};

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub name: String,
    pub limits: wgpu::Limits,
}

impl Gpu {
    /// The first Vulkan GPU, or the one whose name contains
    /// `WALLPAPER_DEPTH_GPU`.
    pub fn new() -> Result<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let want = std::env::var("WALLPAPER_DEPTH_GPU").ok();
        let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
            .into_iter()
            .filter(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
            .find(|a| want.as_ref().is_none_or(|w| a.get_info().name.contains(w.as_str())))
            .context("no Vulkan GPU")?;
        let limits = adapter.limits();
        let features = adapter.features() & (wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::SHADER_F16 | wgpu::Features::SUBGROUP);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("wallpaper-depth"),
            required_features: features,
            required_limits: limits.clone(),
            ..Default::default()
        }))?;
        Ok(Gpu { device, queue, name: adapter.get_info().name, limits })
    }

    /// Multiply-adds one dispatch or submission may do. msm resets the GPU
    /// when a submission makes no progress for a few 250 ms periods, and the
    /// compositor's frames queue behind ours, so on Adreno heavy kernels are
    /// cut to about 12 ms each; elsewhere they stay whole. Every cut costs a
    /// drained GPU, about 0.3 ms.
    pub fn dispatch_budget(&self) -> Option<u64> {
        (self.name.contains("Adreno") || self.name.contains("Turnip")).then_some(1 << 32)
    }

    /// Planning options for this device. WALLPAPER_DEPTH_TEX picks the GEMM
    /// operand textures: f16 (default), f32, or off for the buffer kernels.
    pub fn options(&self) -> Result<crate::plan::Options> {
        use crate::tex::TexFormat;
        let tex = match std::env::var("WALLPAPER_DEPTH_TEX").as_deref() {
            Err(_) | Ok("f16") => Some(TexFormat::F16),
            Ok("f32") => Some(TexFormat::F32),
            Ok("off") => None,
            Ok(other) => bail!("WALLPAPER_DEPTH_TEX={other}: use f16, f32 or off"),
        };
        Ok(crate::plan::Options {
            budget: self.dispatch_budget(),
            tex,
            max_tex_dim: self.limits.max_texture_dimension_2d,
            max_tex_layers: self.limits.max_texture_array_layers,
            fuse: std::env::var_os("WALLPAPER_DEPTH_NO_LAZY").is_none(),
        })
    }

    pub fn wait(&self) -> Result<()> {
        self.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .map_err(|e| anyhow::anyhow!("GPU wait failed: {e}"))?;
        Ok(())
    }
}

/// The GPU objects of a program, each made on first use. A run starts
/// dispatching at once; worker threads compile pipelines and allocate
/// memory ahead of it in dispatch order, so most of the setup happens
/// while the GPU is busy with earlier work.
struct Lazy<'a> {
    gpu: &'a Gpu,
    kernels: &'a [Kernel],
    bufs: &'a [Buf],
    texs: &'a [Tex],
    /// Physical buffers: size, the weight written into it, the buffer.
    phys_sizes: Vec<u64>,
    phys_init: Vec<Option<usize>>,
    physical: Vec<OnceLock<wgpu::Buffer>>,
    phys_of: Vec<usize>,
    tex_descs: Vec<TexDesc>,
    tex_init: Vec<Option<usize>>,
    textures: Vec<OnceLock<(wgpu::Texture, wgpu::TextureView)>>,
    tex_of: Vec<usize>,
    /// Distinct shaders: the first kernel using each, then the pipeline.
    unique: Vec<usize>,
    compiled: Vec<OnceLock<wgpu::ComputePipeline>>,
    pipelines: Vec<usize>,
    params: wgpu::Buffer,
    ranges: Vec<(u64, u64)>,
    groups: Vec<OnceLock<(wgpu::BindGroup, Option<wgpu::BindGroup>)>>,
}

impl Lazy<'_> {
    fn buffer(&self, p: usize) -> &wgpu::Buffer {
        self.physical[p].get_or_init(|| {
            let buf = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: self.phys_sizes[p].max(4),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if let Some(words) = self.phys_init[p].and_then(|b| self.bufs[b].init.as_ref()) {
                self.gpu.queue.write_buffer(&buf, 0, bytemuck::cast_slice(words));
            }
            buf
        })
    }

    fn texture(&self, p: usize) -> &wgpu::TextureView {
        &self.textures[p]
            .get_or_init(|| {
                let d = &self.tex_descs[p];
                let size = wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: d.layers };
                let tex = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: match d.format {
                        TexFormat::F16 => wgpu::TextureFormat::Rgba16Float,
                        TexFormat::F32 => wgpu::TextureFormat::Rgba32Float,
                    },
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                if let Some(bytes) = self.tex_init[p].and_then(|t| self.texs[t].init.as_ref()) {
                    self.gpu.queue.write_texture(
                        tex.as_image_copy(),
                        bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(d.width * d.format.texel_bytes() as u32),
                            rows_per_image: Some(d.height),
                        },
                        size,
                    );
                }
                let view = tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                });
                (tex, view)
            })
            .1
    }

    fn pipeline(&self, i: usize) -> &wgpu::ComputePipeline {
        self.compiled[i].get_or_init(|| {
            let k = &self.kernels[self.unique[i]];
            let device = &self.gpu.device;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&k.label),
                source: wgpu::ShaderSource::Wgsl(k.wgsl.as_str().into()),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(&k.label),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        })
    }

    /// Everything dispatch `d` binds.
    fn memory(&self, d: usize) {
        let k = &self.kernels[d];
        for &(b, _) in &k.bufs {
            self.buffer(self.phys_of[b]);
        }
        for &(t, _) in &k.texs {
            self.texture(self.tex_of[t]);
        }
    }

    /// Whether everything dispatch `d` needs exists already.
    fn made(&self, d: usize) -> bool {
        let k = &self.kernels[d];
        self.groups[d].get().is_some()
            || (self.compiled[self.pipelines[d]].get().is_some()
                && k.bufs.iter().all(|&(b, _)| self.physical[self.phys_of[b]].get().is_some())
                && k.texs.iter().all(|&(t, _)| self.textures[self.tex_of[t]].get().is_some()))
    }

    /// Dispatch `d`'s pipeline and bind groups.
    fn ready(&self, d: usize) -> (&wgpu::ComputePipeline, &wgpu::BindGroup, Option<&wgpu::BindGroup>) {
        let pipeline = self.pipeline(self.pipelines[d]);
        let (g0, g1) = self.groups[d].get_or_init(|| {
            let k = &self.kernels[d];
            let device = &self.gpu.device;
            let window = kernels::WINDOW as u64 * 4;
            let (offset, size) = self.ranges[d];
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.params,
                    offset,
                    size: std::num::NonZeroU64::new(size),
                }),
            }];
            for &(b, _) in &k.bufs {
                // Each logical buffer is bound as windows of at most
                // `window` bytes, as the shader declares them.
                let bytes = self.bufs[b].words as u64 * 4;
                let buffer = self.buffer(self.phys_of[b]);
                for w in 0..kernels::windows(self.bufs[b].words) as u64 {
                    entries.push(wgpu::BindGroupEntry {
                        binding: entries.len() as u32,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer,
                            offset: w * window,
                            size: std::num::NonZeroU64::new(window.min(bytes - w * window)),
                        }),
                    });
                }
            }
            let g0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&k.label),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            });
            let g1 = (!k.texs.is_empty()).then(|| {
                let entries: Vec<wgpu::BindGroupEntry> = k
                    .texs
                    .iter()
                    .enumerate()
                    .map(|(i, &(t, _))| wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu::BindingResource::TextureView(self.texture(self.tex_of[t])),
                    })
                    .collect();
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&k.label),
                    layout: &pipeline.get_bind_group_layout(1),
                    entries: &entries,
                })
            });
            (g0, g1)
        });
        (pipeline, g0, g1.as_ref())
    }

    /// Makes everything ahead of the dispatching thread: pipelines on the
    /// worker threads, memory on one more. Returns once all of it exists.
    fn prefetch(&self) {
        if self.groups.iter().all(|g| g.get().is_some()) {
            return;
        }
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..crate::tensor::workers() {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= self.unique.len() {
                            break;
                        }
                        self.pipeline(i);
                    }
                });
            }
            s.spawn(|| (0..self.kernels.len()).for_each(|d| self.memory(d)));
        });
    }
}

/// The most bytes alive at once, from (first, last, bytes) lifetimes in
/// dispatches.
fn peak(end: usize, spans: impl Iterator<Item = (usize, usize, u64)>) -> u64 {
    let mut live = vec![0i64; end + 2];
    for (first, last, bytes) in spans {
        live[first] += bytes as i64;
        live[last + 1] -= bytes as i64;
    }
    live.iter()
        .scan(0i64, |acc, &d| {
            *acc += d;
            Some(*acc)
        })
        .max()
        .unwrap_or(0) as u64
}

pub struct Runner<'a> {
    gpu: &'a Gpu,
    program: &'a Program,
    lazy: Lazy<'a>,
    /// Where a run is cut into separate submissions.
    batches: Vec<std::ops::Range<usize>>,
    pub weight_bytes: u64,
    pub scratch_bytes: u64,
    pub peak_live: u64,
}

impl<'a> Runner<'a> {
    /// Plans memory, pipelines and submissions; makes no GPU objects yet.
    pub fn new(gpu: &'a Gpu, program: &'a Program) -> Result<Runner<'a>> {
        let n = program.bufs.len();
        let end = program.kernels.len();
        let (mut first, mut last) = (vec![usize::MAX; n], vec![0; n]);
        for (d, k) in program.kernels.iter().enumerate() {
            for &(b, _) in &k.bufs {
                first[b] = first[b].min(d);
                last[b] = d;
            }
        }
        // A step's inputs live until its last dispatch, so node checks can
        // still read them after the step.
        for step in &program.steps {
            if step.dispatches.is_empty() {
                continue;
            }
            for v in step.inputs.iter().flatten() {
                if let Val::Gpu(v) = v
                    && first[v.buf] != usize::MAX
                {
                    last[v.buf] = last[v.buf].max(step.dispatches.end - 1);
                }
            }
        }
        first[program.input.buf] = 0;
        for v in &program.outputs {
            if let Val::Gpu(v) = v {
                last[v.buf] = end;
            }
        }
        // Weights get their own buffers. Scratch buffers, largest first, take
        // the smallest physical buffer none of whose tenants is alive at the
        // same time; sorted that way, no buffer ever has to grow.
        let mut sizes: Vec<u64> = Vec::new();
        let mut phys_init: Vec<Option<usize>> = Vec::new();
        let mut phys_of = vec![usize::MAX; n];
        let mut scratch = Vec::new();
        for (b, buf) in program.bufs.iter().enumerate() {
            if buf.init.is_some() {
                phys_of[b] = sizes.len();
                sizes.push(buf.words as u64 * 4);
                phys_init.push(Some(b));
            } else if first[b] != usize::MAX {
                scratch.push(b);
            }
        }
        scratch.sort_by_key(|&b| std::cmp::Reverse(program.bufs[b].words));
        let mut tenants: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        for b in scratch {
            let need = program.bufs[b].words as u64 * 4;
            let span = (first[b], last[b]);
            let free = |p: &usize| tenants[p].iter().all(|&(s, e)| e < span.0 || s > span.1);
            let p = match tenants.keys().copied().filter(free).min_by_key(|&p| sizes[p]) {
                Some(p) => p,
                None => {
                    sizes.push(need);
                    phys_init.push(None);
                    sizes.len() - 1
                }
            };
            tenants.entry(p).or_default().push(span);
            phys_of[b] = p;
        }
        let peak_live = peak(
            end,
            program
                .bufs
                .iter()
                .enumerate()
                .filter(|&(b, buf)| buf.init.is_none() && first[b] != usize::MAX)
                .map(|(b, buf)| (first[b], last[b], buf.words as u64 * 4)),
        );
        let window = kernels::WINDOW as u64 * 4;
        ensure!(
            gpu.limits.max_storage_buffer_binding_size >= window,
            "bindings are limited to {} bytes",
            gpu.limits.max_storage_buffer_binding_size
        );
        if let Some(big) = sizes.iter().find(|&&s| s > gpu.limits.max_buffer_size) {
            bail!("a {big}-byte buffer exceeds the device limit of {}", gpu.limits.max_buffer_size);
        }
        let mut weight_bytes: u64 = sizes.iter().zip(&phys_init).filter(|(_, w)| w.is_some()).map(|(s, _)| s).sum();
        let mut scratch_bytes: u64 = sizes.iter().zip(&phys_init).filter(|(_, w)| w.is_none()).map(|(s, _)| s).sum();

        // Textures: weights get their own. Scratch textures, largest first,
        // take the smallest free texture that contains them, or grow a free
        // one when that costs less than a new texture.
        let nt = program.textures.len();
        let (mut tfirst, mut tlast) = (vec![usize::MAX; nt], vec![0; nt]);
        for (d, k) in program.kernels.iter().enumerate() {
            for &(t, _) in &k.texs {
                tfirst[t] = tfirst[t].min(d);
                tlast[t] = d;
            }
        }
        let mut tex_descs: Vec<TexDesc> = Vec::new();
        let mut tex_init: Vec<Option<usize>> = Vec::new();
        let mut tex_of = vec![usize::MAX; nt];
        let mut tex_tenants: Vec<Vec<(usize, usize)>> = Vec::new();
        let mut order: Vec<usize> = (0..nt).filter(|&t| tfirst[t] != usize::MAX).collect();
        order.sort_by_key(|&t| (program.textures[t].init.is_none(), std::cmp::Reverse(program.textures[t].desc.bytes())));
        for t in order {
            let tx = &program.textures[t];
            if tx.init.is_some() {
                tex_descs.push(tx.desc);
                tex_init.push(Some(t));
                tex_tenants.push(Vec::new());
                weight_bytes += tx.desc.bytes();
                tex_of[t] = tex_descs.len() - 1;
                continue;
            }
            let span = (tfirst[t], tlast[t]);
            let free: Vec<usize> = (0..tex_descs.len())
                .filter(|&p| tex_init[p].is_none() && tex_tenants[p].iter().all(|&(s, e)| e < span.0 || s > span.1))
                .collect();
            let p = if let Some(&p) = free.iter().filter(|&&p| tex_descs[p].contains(&tx.desc)).min_by_key(|&&p| tex_descs[p].bytes()) {
                p
            } else if let Some(&p) = free
                .iter()
                .filter(|&&p| tex_descs[p].format == tx.desc.format)
                .filter(|&&p| tex_descs[p].union(&tx.desc).bytes() <= tex_descs[p].bytes() + tx.desc.bytes())
                .min_by_key(|&&p| tex_descs[p].union(&tx.desc).bytes())
            {
                tex_descs[p] = tex_descs[p].union(&tx.desc);
                p
            } else {
                tex_descs.push(tx.desc);
                tex_init.push(None);
                tex_tenants.push(Vec::new());
                tex_descs.len() - 1
            };
            tex_tenants[p].push(span);
            tex_of[t] = p;
        }
        for d in &tex_descs {
            ensure!(
                d.width <= gpu.limits.max_texture_dimension_2d
                    && d.height <= gpu.limits.max_texture_dimension_2d
                    && d.layers <= gpu.limits.max_texture_array_layers,
                "texture {d:?} exceeds the device limits"
            );
        }
        let tex_peak = peak(
            end,
            (0..nt)
                .filter(|&t| program.textures[t].init.is_none() && tfirst[t] != usize::MAX)
                .map(|t| (tfirst[t], tlast[t], program.textures[t].desc.bytes())),
        );
        if std::env::var_os("WALLPAPER_DEPTH_VERBOSE").is_some() {
            let tex_bytes: u64 = tex_descs.iter().zip(&tex_init).filter(|(_, i)| i.is_none()).map(|(d, _)| d.bytes()).sum();
            eprintln!(
                "scratch buffers {:.1} MB (peak live {:.1} MB), textures {:.1} MB (peak live {:.1} MB)",
                scratch_bytes as f64 / 1e6,
                peak_live as f64 / 1e6,
                tex_bytes as f64 / 1e6,
                tex_peak as f64 / 1e6
            );
        }
        scratch_bytes += tex_descs.iter().zip(&tex_init).filter(|(_, i)| i.is_none()).map(|(d, _)| d.bytes()).sum::<u64>();

        // One pipeline per distinct shader.
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut unique = Vec::new();
        let mut pipelines = Vec::with_capacity(end);
        for (d, k) in program.kernels.iter().enumerate() {
            let next = unique.len();
            let i = *index.entry(k.wgsl.as_str()).or_insert(next);
            if i == next {
                unique.push(d);
            }
            pipelines.push(i);
        }
        let align = u64::from(gpu.limits.min_storage_buffer_offset_alignment);
        let mut params: Vec<u8> = Vec::new();
        let mut ranges = Vec::with_capacity(end);
        for k in &program.kernels {
            let start = params.len() as u64;
            let words: &[u32] = if k.params.is_empty() { &[0] } else { &k.params };
            params.extend(bytemuck::cast_slice(words));
            ranges.push((start, (words.len() * 4) as u64));
            params.resize((params.len() as u64).next_multiple_of(align) as usize, 0);
        }
        let params_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: (params.len() as u64).max(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&params_buf, 0, &params);

        // Under a dispatch budget, a submission holds at most a budget's worth
        // of work (or one dispatch), so the hang check sees the GPU finish
        // them regularly and other clients get the GPU between them.
        let mut batches = Vec::new();
        let mut start = 0;
        let mut work = 0u64;
        for (d, k) in program.kernels.iter().enumerate() {
            if let Some(budget) = gpu.dispatch_budget()
                && work + k.work > budget
                && d > start
            {
                batches.push(start..d);
                (start, work) = (d, 0);
            }
            work += k.work;
        }
        batches.push(start..end);
        let lazy = Lazy {
            gpu,
            kernels: &program.kernels,
            bufs: &program.bufs,
            texs: &program.textures,
            physical: sizes.iter().map(|_| OnceLock::new()).collect(),
            phys_sizes: sizes,
            phys_init,
            phys_of,
            textures: tex_descs.iter().map(|_| OnceLock::new()).collect(),
            tex_descs,
            tex_init,
            tex_of,
            compiled: unique.iter().map(|_| OnceLock::new()).collect(),
            unique,
            pipelines,
            params: params_buf,
            ranges,
            groups: (0..end).map(|_| OnceLock::new()).collect(),
        };
        Ok(Runner { gpu, program, lazy, batches, weight_bytes, scratch_bytes, peak_live })
    }

    pub fn pipeline_count(&self) -> usize {
        self.lazy.unique.len()
    }

    pub fn submissions(&self) -> usize {
        self.batches.len()
    }

    fn upload_input(&self, input: &Tensor) -> Result<()> {
        let v = &self.program.input;
        ensure!(input.shape == v.shape, "input {:?} for a program planned for {:?}", input.shape, v.shape);
        self.gpu.queue.write_buffer(self.lazy.buffer(self.lazy.phys_of[v.buf]), v.offset as u64 * 4, bytemuck::cast_slice(input.as_f32()?));
        Ok(())
    }

    /// Encodes and submits a batch. Whatever is encoded goes to the GPU
    /// before waiting for a dispatch whose objects are still being made, so
    /// on a first run the GPU starts while memory is still being allocated.
    fn dispatch(&self, range: std::ops::Range<usize>) {
        let mut d = range.start;
        while d < range.end {
            let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                let start = d;
                while d < range.end && (d == start || self.lazy.made(d)) {
                    let k = &self.program.kernels[d];
                    let (pipeline, g0, g1) = self.lazy.ready(d);
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, g0, &[]);
                    if let Some(g) = g1 {
                        pass.set_bind_group(1, g, &[]);
                    }
                    pass.dispatch_workgroups(k.groups[0], k.groups[1], k.groups[2]);
                    d += 1;
                }
            }
            self.gpu.queue.submit([encoder.finish()]);
        }
    }

    /// One whole run; returns the outputs and the time from upload to the
    /// GPU going idle.
    pub fn run(&self, input: &Tensor) -> Result<(Vec<Tensor>, Duration)> {
        let lazy = &self.lazy;
        let started = Instant::now();
        // The input goes first: once the prefetch thread allocates, the
        // staging memory for the upload can wait behind all of it.
        self.upload_input(input)?;
        std::thread::scope(|s| {
            s.spawn(move || lazy.prefetch());
            for batch in &self.batches {
                self.dispatch(batch.clone());
            }
        });
        self.gpu.wait()?;
        let elapsed = started.elapsed();
        let outs = self
            .program
            .outputs
            .iter()
            .map(|v| match v {
                Val::Gpu(v) => self.download(v),
                Val::Const(t) => Ok((**t).clone()),
                Val::Lazy(_) | Val::Rows(_) | Val::Cat(_) => bail!("an output was never computed"),
            })
            .collect::<Result<_>>()?;
        Ok((outs, elapsed))
    }

    /// Times every dispatch and returns (label, seconds) pairs. With GPU
    /// timestamps the run keeps its batches and the times are the GPU's own;
    /// without them, or under WALLPAPER_DEPTH_TRACE, every dispatch is its
    /// own submission and the times include the round trip.
    pub fn profile(&self, input: &Tensor) -> Result<Vec<(String, f64)>> {
        self.upload_input(input)?;
        self.gpu.wait()?;
        let trace = std::env::var_os("WALLPAPER_DEPTH_TRACE").is_some();
        if !trace && self.gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return self.profile_timestamps();
        }
        let mut times = Vec::with_capacity(self.program.kernels.len());
        for (d, k) in self.program.kernels.iter().enumerate() {
            if trace {
                use std::io::Write;
                println!("{d} {} groups {:?}", k.label, k.groups);
                std::io::stdout().flush()?;
            }
            let started = Instant::now();
            self.dispatch(d..d + 1);
            self.gpu.wait()?;
            times.push((k.label.clone(), started.elapsed().as_secs_f64()));
        }
        Ok(times)
    }

    fn profile_timestamps(&self) -> Result<Vec<(String, f64)>> {
        // Every timestamped pass waits for the GPU to drain; a submission of
        // a thousand small ones outlasts msm's hang check.
        const PER: usize = 128;
        let device = &self.gpu.device;
        let period = f64::from(self.gpu.queue.get_timestamp_period()) * 1e-9;
        let mut times = Vec::with_capacity(self.program.kernels.len());
        for batch in &self.batches {
            for start in batch.clone().step_by(PER) {
                let range = start..(start + PER).min(batch.end);
                let count = (range.len() * 2) as u32;
                let queries = device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count });
                let size = u64::from(count) * 8;
                let resolved = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let staging = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                for (i, d) in range.clone().enumerate() {
                    let k = &self.program.kernels[d];
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: None,
                        timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                            query_set: &queries,
                            beginning_of_pass_write_index: Some(2 * i as u32),
                            end_of_pass_write_index: Some(2 * i as u32 + 1),
                        }),
                    });
                    let (pipeline, g0, g1) = self.lazy.ready(d);
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, g0, &[]);
                    if let Some(g) = g1 {
                        pass.set_bind_group(1, g, &[]);
                    }
                    pass.dispatch_workgroups(k.groups[0], k.groups[1], k.groups[2]);
                }
                encoder.resolve_query_set(&queries, 0..count, &resolved, 0);
                encoder.copy_buffer_to_buffer(&resolved, 0, &staging, 0, size);
                self.gpu.queue.submit([encoder.finish()]);
                staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                self.gpu.wait()?;
                let ticks: Vec<u64> = bytemuck::cast_slice(&staging.slice(..).get_mapped_range()?).to_vec();
                for (i, d) in range.enumerate() {
                    let t = ticks[2 * i + 1].saturating_sub(ticks[2 * i]) as f64 * period;
                    times.push((self.program.kernels[d].label.clone(), t));
                }
            }
        }
        Ok(times)
    }

    pub fn download(&self, v: &View) -> Result<Tensor> {
        let n = v.numel();
        if n == 0 {
            return Ok(match v.dtype {
                DType::F32 => Tensor::f32(v.shape.clone(), vec![]),
                DType::I64 => Tensor::i64(v.shape.clone(), vec![]),
                DType::Bool => Tensor::bool(v.shape.clone(), vec![]),
            });
        }
        let (mut lo, mut hi) = (v.offset as isize, v.offset as isize);
        for (&d, &s) in v.shape.iter().zip(&v.strides) {
            let reach = (d as isize - 1) * s;
            if reach < 0 { lo += reach } else { hi += reach }
        }
        let bytes = (hi - lo + 1) as u64 * 4;
        let staging = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("download"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(self.lazy.buffer(self.lazy.phys_of[v.buf]), lo as u64 * 4, &staging, 0, bytes);
        self.gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.gpu.wait()?;
        rx.recv()?.map_err(|e| anyhow::anyhow!("mapping failed: {e}"))?;
        let words: Vec<u32> = bytemuck::cast_slice(&staging.slice(..).get_mapped_range()?).to_vec();
        let idx = offsets(&v.shape, (v.offset as isize - lo) as usize, &v.strides);
        Ok(match v.dtype {
            DType::F32 => Tensor::f32(v.shape.clone(), idx.iter().map(|&i| f32::from_bits(words[i])).collect()),
            DType::I64 => Tensor::i64(v.shape.clone(), idx.iter().map(|&i| i64::from(words[i] as i32)).collect()),
            DType::Bool => Tensor::bool(v.shape.clone(), idx.iter().map(|&i| words[i] != 0).collect()),
        })
    }

    /// Runs node by node and compares every GPU result with the reference op
    /// fed the same GPU inputs. Prints the nodes that disagree.
    pub fn check_nodes(&self, input: &Tensor, filter: Option<&str>) -> Result<()> {
        self.upload_input(input)?;
        let mut worst = (0.0f64, String::new());
        let mut bad = 0;
        for step in &self.program.steps {
            self.dispatch(step.dispatches.clone());
            self.gpu.wait()?;
            let node = &self.program.nodes[step.node];
            if filter.is_some_and(|f| !node.op.contains(f)) {
                continue;
            }
            let ins: Vec<Option<Tensor>> = step
                .inputs
                .iter()
                .map(|v| match v {
                    None => Ok(None),
                    Some(Val::Const(t)) => Ok(Some((**t).clone())),
                    Some(Val::Gpu(v)) => self.download(v).map(Some),
                    Some(Val::Lazy(_) | Val::Rows(_) | Val::Cat(_)) => bail!("node checks need WALLPAPER_DEPTH_NO_LAZY"),
                })
                .collect::<Result<_>>()?;
            let refs: Vec<Option<&Tensor>> = ins.iter().map(Option::as_ref).collect();
            let expected = reference::eval(node, &refs, self.program.opset).with_context(|| format!("reference for {}", node.label()))?;
            for (o, (got, want)) in step.outputs.iter().zip(&expected).enumerate() {
                let got = match got {
                    Val::Gpu(v) => self.download(v)?,
                    Val::Const(t) => (**t).clone(),
                    Val::Lazy(_) | Val::Rows(_) | Val::Cat(_) => bail!("node checks need WALLPAPER_DEPTH_NO_LAZY"),
                };
                let (err, scale) = compare(&got, want)?;
                let rel = err / scale.max(1.0);
                if rel > worst.0 {
                    worst = (rel, node.label());
                }
                if rel > 1e-3 {
                    bad += 1;
                    println!("MISMATCH {} out {o}: max err {err:.3e}, max |ref| {scale:.3e}, shape {:?}", node.label(), want.shape);
                }
            }
        }
        println!("checked {} steps, {bad} mismatches, worst relative error {:.3e} at {}", self.program.steps.len(), worst.0, worst.1);
        Ok(())
    }
}

/// Largest absolute difference and largest reference magnitude.
pub fn compare(got: &Tensor, want: &Tensor) -> Result<(f64, f64)> {
    ensure!(got.shape == want.shape, "shape {:?} instead of {:?}", got.shape, want.shape);
    ensure!(numel(&got.shape) == got.data_len(), "malformed tensor");
    let (g, w) = (got.to_f32(), want.to_f32());
    let mut err = 0f64;
    let mut scale = 0f64;
    for (&a, &b) in g.iter().zip(&w) {
        let d = (f64::from(a) - f64::from(b)).abs();
        err = if d.is_nan() { f64::INFINITY } else { err.max(d) };
        scale = scale.max(f64::from(b).abs());
    }
    Ok((err, scale))
}
