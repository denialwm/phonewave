//! GEMM kernel experiments: C[M,N] = A[M,K] B[K,N], all row-major, timed
//! with GPU timestamps and spot-checked against the CPU.

use anyhow::{Context, Result, ensure};

use crate::gpu::Gpu;

pub struct Variant {
    pub name: String,
    pub wgsl: String,
    pub groups: [u32; 3],
    /// A and B are f16.
    pub half: bool,
    /// A and B are textures of this format instead of buffers: A is K/4
    /// texels wide and M high, B N/4 wide and K high.
    pub texture: Option<wgpu::TextureFormat>,
}

fn shared64(m: usize, n: usize, k: usize) -> Variant {
    let wgsl = format!(
        "
@group(0) @binding(0) var<storage, read> A: array<f32>;
@group(0) @binding(1) var<storage, read> B: array<f32>;
@group(0) @binding(2) var<storage, read_write> C: array<f32>;
const M = {m}u; const N = {n}u; const K = {k}u;
var<workgroup> As: array<vec4<f32>, 256>;
var<workgroup> Bs: array<vec4<f32>, 256>;
@compute @workgroup_size(16, 16, 1)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(local_invocation_index) li: u32) {{
    let m0 = wg.y * 64u;
    let n0 = wg.x * 64u;
    let ak = li % 16u;
    let bn = li % 64u;
    let nb = n0 + bn;
    var acc0 = vec4<f32>(0.0);
    var acc1 = vec4<f32>(0.0);
    var acc2 = vec4<f32>(0.0);
    var acc3 = vec4<f32>(0.0);
    for (var k0 = 0u; k0 < K; k0 += 16u) {{
        for (var t = 0u; t < 4u; t++) {{
            let am = li / 16u + t * 16u;
            var av = 0.0;
            if (m0 + am < M && k0 + ak < K) {{ av = A[(m0 + am) * K + k0 + ak]; }}
            As[ak * 16u + am / 4u][am % 4u] = av;
            let bk = li / 64u + t * 4u;
            var bv = 0.0;
            if (k0 + bk < K && nb < N) {{ bv = B[(k0 + bk) * N + nb]; }}
            Bs[bk * 16u + bn / 4u][bn % 4u] = bv;
        }}
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {{
            let a = As[kk * 16u + lid.y];
            let b = Bs[kk * 16u + lid.x];
            acc0 += a.x * b;
            acc1 += a.y * b;
            acc2 += a.z * b;
            acc3 += a.w * b;
        }}
        workgroupBarrier();
    }}
    let m = m0 + lid.y * 4u;
    let n = n0 + lid.x * 4u;
    let rows = array<vec4<f32>, 4>(acc0, acc1, acc2, acc3);
    for (var i = 0u; i < 4u; i++) {{
        if (m + i >= M) {{ break; }}
        for (var j = 0u; j < 4u; j++) {{
            if (n + j < N) {{ C[(m + i) * N + n + j] = rows[i][j]; }}
        }}
    }}
}}"
    );
    Variant { name: "shared64".into(), wgsl, groups: [n.div_ceil(64) as u32, m.div_ceil(64) as u32, 1], half: false, texture: None }
}

/// No shared memory: each thread keeps an R x (4*V) block of C in
/// registers and reads A and B as vec4 straight from the buffers, leaning on
/// the caches for reuse. Needs K and N to be multiples of 4.
fn direct(m: usize, n: usize, k: usize, r: usize, v: usize, wx: usize, wy: usize) -> Variant {
    let mut decl = String::new();
    let mut fma = String::new();
    let mut store = String::new();
    for i in 0..r {
        for j in 0..v {
            decl += &format!("    var c{i}_{j} = vec4<f32>(0.0);\n");
            store += &format!("    if (m + {i}u < M && n + {j}u < N4) {{ C4[(m + {i}u) * N4 + n + {j}u] = c{i}_{j}; }}\n");
        }
    }
    // Per k-step of 4: rows of A as vec4 over k, rows of B as vec4 over n.
    for i in 0..r {
        fma += &format!("        let a{i} = A4[min(m + {i}u, M - 1u) * K4 + k];\n");
    }
    for q in 0..4 {
        for j in 0..v {
            fma += &format!("        let b{q}_{j} = B4[(k * 4u + {q}u) * N4 + min(n + {j}u, N4 - 1u)];\n");
        }
    }
    for i in 0..r {
        for j in 0..v {
            fma += &format!(
                "        c{i}_{j} = fma(vec4(a{i}.x), b0_{j}, fma(vec4(a{i}.y), b1_{j}, fma(vec4(a{i}.z), b2_{j}, fma(vec4(a{i}.w), b3_{j}, c{i}_{j}))));\n"
            );
        }
    }
    let wgsl = format!(
        "
@group(0) @binding(0) var<storage, read> A4: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> B4: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> C4: array<vec4<f32>>;
const M = {m}u; const N4 = {n4}u; const K4 = {k4}u;
@compute @workgroup_size({wx}, {wy}, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
    let n = gid.x * {v}u;
    let m = gid.y * {r}u;
{decl}    for (var k = 0u; k < K4; k++) {{
{fma}    }}
{store}}}",
        n4 = n / 4,
        k4 = k / 4,
    );
    Variant {
        name: format!("direct{r}x{}_{wx}x{wy}", 4 * v),
        wgsl,
        groups: [(n / 4).div_ceil(v * wx) as u32, m.div_ceil(r * wy) as u32, 1],
        half: false,
        texture: None,
    }
}

/// Builds a WGSL operand read from a row and a column expression.
type Fetch = Box<dyn Fn(String, String) -> String>;

/// `direct` with operands read through `fetch`: "tex" reads textures,
/// "pk16" unpacks f16 pairs from u32 buffers.
fn direct_fetch(m: usize, n: usize, k: usize, r: usize, v: usize, wx: usize, wy: usize, fetch: &str) -> Variant {
    let mut decl = String::new();
    let mut store = String::new();
    let mut fma = String::new();
    for i in 0..r {
        for j in 0..v {
            decl += &format!("    var c{i}_{j} = vec4<f32>(0.0);\n");
            store += &format!("    if (m + {i}u < M && n + {j}u < N4) {{ C4[(m + {i}u) * N4 + n + {j}u] = c{i}_{j}; }}\n");
        }
    }
    let (bind, a_at, b_at): (&str, Fetch, Fetch) = match fetch {
        "tex" => (
            "@group(0) @binding(0) var At: texture_2d<f32>;\n@group(0) @binding(1) var Bt: texture_2d<f32>;\n",
            Box::new(|row, col| format!("textureLoad(At, vec2<u32>({col}, {row}), 0)")),
            Box::new(|row, col| format!("textureLoad(Bt, vec2<u32>({col}, {row}), 0)")),
        ),
        _ => (
            "@group(0) @binding(0) var<storage, read> Ap: array<vec2<u32>>;\n@group(0) @binding(1) var<storage, read> Bp: array<vec2<u32>>;\n",
            Box::new(|row, col| format!("unpack4(Ap[({row}) * K4 + {col}])")),
            Box::new(|row, col| format!("unpack4(Bp[({row}) * N4 + {col}])")),
        ),
    };
    for i in 0..r {
        fma += &format!("        let a{i} = {};\n", a_at(format!("min(m + {i}u, M - 1u)"), "k".into()));
    }
    for q in 0..4 {
        for j in 0..v {
            fma += &format!("        let b{q}_{j} = {};\n", b_at(format!("k * 4u + {q}u"), format!("min(n + {j}u, N4 - 1u)")));
        }
    }
    for i in 0..r {
        for j in 0..v {
            fma += &format!(
                "        c{i}_{j} = fma(vec4(a{i}.x), b0_{j}, fma(vec4(a{i}.y), b1_{j}, fma(vec4(a{i}.z), b2_{j}, fma(vec4(a{i}.w), b3_{j}, c{i}_{j}))));\n"
            );
        }
    }
    let wgsl = format!(
        "
{bind}@group(0) @binding(2) var<storage, read_write> C4: array<vec4<f32>>;
const M = {m}u; const N4 = {n4}u; const K4 = {k4}u;
fn unpack4(x: vec2<u32>) -> vec4<f32> {{ return vec4(unpack2x16float(x.x), unpack2x16float(x.y)); }}
@compute @workgroup_size({wx}, {wy}, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
    let n = gid.x * {v}u;
    let m = gid.y * {r}u;
{decl}    for (var k = 0u; k < K4; k++) {{
{fma}    }}
{store}}}",
        n4 = n / 4,
        k4 = k / 4,
    );
    let half = fetch == "pk16";
    Variant {
        name: format!("{fetch}_{r}x{}_{wx}x{wy}", 4 * v),
        wgsl,
        groups: [(n / 4).div_ceil(v * wx) as u32, m.div_ceil(r * wy) as u32, 1],
        half,
        texture: None,
    }
}

fn variants(m: usize, n: usize, k: usize) -> Vec<Variant> {
    let mut out = vec![shared64(m, n, k)];
    if n.is_multiple_of(4) && k.is_multiple_of(4) {
        out.push(direct(m, n, k, 8, 2, 16, 4));
        for (r, v) in [(4, 1), (4, 2), (8, 1), (8, 2)] {
            for (wx, wy) in [(16, 4), (8, 8)] {
                for format in [wgpu::TextureFormat::Rgba32Float, wgpu::TextureFormat::Rgba16Float] {
                    let mut t = direct_fetch(m, n, k, r, v, wx, wy, "tex");
                    t.name = format!("{}{}", t.name, if format == wgpu::TextureFormat::Rgba32Float { "_f32" } else { "_f16" });
                    t.texture = Some(format);
                    out.push(t);
                }
            }
        }
    }
    out
}

pub fn gemm(gpu: &Gpu, m: usize, n: usize, k: usize, filter: Option<&str>) -> Result<()> {
    ensure!(gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY), "no GPU timestamps");
    let device = &gpu.device;
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    let a: Vec<f32> = (0..m * k).map(|_| rand()).collect();
    let b: Vec<f32> = (0..k * n).map(|_| rand()).collect();
    let storage = |data: &[f32]| {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (data.len() * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&buf, 0, bytemuck::cast_slice(data));
        buf
    };
    let (ab, bb) = (storage(&a), storage(&b));
    let storage_h = |data: &[f32]| {
        let h: Vec<u16> = data.iter().map(|&x| half::f16::from_f32(x).to_bits()).collect();
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (h.len() * 2).next_multiple_of(4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&buf, 0, bytemuck::cast_slice(&h));
        buf
    };
    let (ah, bh) = (storage_h(&a), storage_h(&b));
    let texture = |data: &[f32], w: usize, h: usize, format: wgpu::TextureFormat| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes: Vec<u8> = if format == wgpu::TextureFormat::Rgba32Float {
            bytemuck::cast_slice(data).to_vec()
        } else {
            let h: Vec<u16> = data.iter().map(|&x| half::f16::from_f32(x).to_bits()).collect();
            bytemuck::cast_slice(&h).to_vec()
        };
        let texel = if format == wgpu::TextureFormat::Rgba32Float { 16 } else { 8 };
        gpu.queue.write_texture(
            tex.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some((w * texel) as u32), rows_per_image: None },
            wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        );
        tex.create_view(&Default::default())
    };
    let mut textures: Vec<(wgpu::TextureFormat, wgpu::TextureView, wgpu::TextureView)> = Vec::new();
    let cb = storage(&vec![0f32; m * n]);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (m * n * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 2 });
    let resolved = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let ts = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let period = f64::from(gpu.queue.get_timestamp_period()) * 1e-9;
    let flops = 2.0 * (m * n * k) as f64;
    println!("{}: M {m} N {n} K {k}, {:.2} GFLOP", gpu.name, flops / 1e9);
    let mut results = Vec::new();
    for v in variants(m, n, k) {
        if filter.is_some_and(|f| !v.name.contains(f))
            || (v.wgsl.contains("enable f16") && !device.features().contains(wgpu::Features::SHADER_F16))
        {
            continue;
        }
        if let Some(format) = v.texture
            && !textures.iter().any(|t| t.0 == format)
        {
            textures.push((format, texture(&a, k / 4, m, format), texture(&b, n / 4, k, format)));
        }
        let (ab, bb) = if v.half { (&ah, &bh) } else { (&ab, &bb) };
        let views = v.texture.and_then(|f| textures.iter().find(|t| t.0 == f));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&v.name),
            source: wgpu::ShaderSource::Wgsl(v.wgsl.as_str().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(&v.name),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: views.map_or(ab.as_entire_binding(), |t| wgpu::BindingResource::TextureView(&t.1)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: views.map_or(bb.as_entire_binding(), |t| wgpu::BindingResource::TextureView(&t.2)),
                },
                wgpu::BindGroupEntry { binding: 2, resource: cb.as_entire_binding() },
            ],
        });
        let mut best = f64::INFINITY;
        for _ in 0..4 {
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.clear_buffer(&cb, 0, None);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                        query_set: &queries,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    }),
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(v.groups[0], v.groups[1], v.groups[2]);
            }
            encoder.resolve_query_set(&queries, 0..2, &resolved, 0);
            encoder.copy_buffer_to_buffer(&resolved, 0, &ts, 0, 16);
            gpu.queue.submit([encoder.finish()]);
            ts.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            gpu.wait()?;
            let t: Vec<u64> = bytemuck::cast_slice(&ts.slice(..).get_mapped_range()?).to_vec();
            ts.unmap();
            best = best.min(t[1].saturating_sub(t[0]) as f64 * period);
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&cb, 0, &staging, 0, (m * n * 4) as u64);
        gpu.queue.submit([encoder.finish()]);
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        gpu.wait()?;
        let c: Vec<f32> = bytemuck::cast_slice(&staging.slice(..).get_mapped_range()?).to_vec();
        staging.unmap();
        let mut err = 0f64;
        for s in 0..256 {
            let (i, j) = ((s * 7919) % m, (s * 104729) % n);
            let want: f64 = (0..k).map(|q| f64::from(a[i * k + q]) * f64::from(b[q * n + j])).sum();
            err = err.max((f64::from(c[i * n + j]) - want).abs());
        }
        println!("{:>24} {:>8.3} ms {:>7.0} GFLOPS  err {err:.1e}", v.name, best * 1e3, flops / best / 1e9);
        results.push((best, v.name));
    }
    results.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (t, name) = results.first().context("no variant ran")?;
    println!("best {name}: {:.0} GFLOPS", flops / t / 1e9);
    Ok(())
}
