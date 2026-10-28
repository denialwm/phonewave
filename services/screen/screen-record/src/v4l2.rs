//! The SoC's H.264 encoder through the V4L2 stateful encoder interface.
//!
//! The device allocates its NV12 input buffers and exports them as
//! DMA-BUFs, which the GPU renders into, so no frame passes through the CPU.
//! The H.264 stream comes out of the device's own buffers.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const NV12: u32 = u32::from_le_bytes(*b"NV12");
const H264: u32 = u32::from_le_bytes(*b"H264");

/// Raw frames in.
const OUTPUT_MPLANE: u32 = 10;
/// Bitstream out.
const CAPTURE_MPLANE: u32 = 9;
/// Selections take the single-plane buffer type.
const OUTPUT: u32 = 2;
const MEMORY_MMAP: u32 = 1;
const FIELD_NONE: u32 = 1;
const CAP_VIDEO_M2M_MPLANE: u32 = 0x4000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const BUF_FLAG_LAST: u32 = 0x0010_0000;
const SEL_TGT_CROP: u32 = 0;
const ENC_CMD_STOP: u32 = 1;
const COLORSPACE_REC709: u32 = 3;
const YCBCR_ENC_709: u8 = 2;
const QUANTIZATION_LIM_RANGE: u8 = 2;
const XFER_FUNC_709: u8 = 1;

const CID_CODEC_BASE: u32 = 0x0099_0900;
const CID_B_FRAMES: u32 = CID_CODEC_BASE + 202;
const CID_GOP_SIZE: u32 = CID_CODEC_BASE + 203;
const CID_BITRATE_MODE: u32 = CID_CODEC_BASE + 206;
const CID_BITRATE: u32 = CID_CODEC_BASE + 207;
const CID_H264_LEVEL: u32 = CID_CODEC_BASE + 359;
const CID_H264_PROFILE: u32 = CID_CODEC_BASE + 363;
const BITRATE_MODE_VBR: i32 = 0;
/// Level 5.1: 1080x2376 at 60 fps is past level 5's macroblock rate.
const H264_LEVEL_5_1: i32 = 15;
const H264_PROFILE_HIGH: i32 = 4;

/// Input buffers: one shown, one being drawn, the rest in the encoder.
const INPUTS: u32 = 5;
const OUTPUTS: u32 = 4;
/// A frame the encoder holds longer than this means it has stalled.
const STALL: Duration = Duration::from_secs(1);

#[repr(C)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
struct FmtDesc {
    index: u32,
    kind: u32,
    flags: u32,
    description: [u8; 32],
    pixelformat: u32,
    mbus_code: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PlaneFormat {
    sizeimage: u32,
    bytesperline: u32,
    reserved: [u16; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PixFormatMplane {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    colorspace: u32,
    plane_fmt: [PlaneFormat; 8],
    num_planes: u8,
    flags: u8,
    ycbcr_enc: u8,
    quantization: u8,
    xfer_func: u8,
    reserved: [u8; 7],
}

#[repr(C)]
union FormatData {
    pix_mp: PixFormatMplane,
    raw: [u64; 25],
}

#[repr(C)]
struct Format {
    kind: u32,
    fmt: FormatData,
}

#[repr(C)]
struct RequestBuffers {
    count: u32,
    kind: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[repr(C)]
struct Plane {
    bytesused: u32,
    length: u32,
    /// mem_offset in its low half for MMAP buffers.
    m: u64,
    data_offset: u32,
    reserved: [u32; 11],
}

#[repr(C)]
struct Buffer {
    index: u32,
    kind: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: [u32; 4],
    sequence: u32,
    memory: u32,
    planes: *mut Plane,
    length: u32,
    reserved2: u32,
    request_fd: i32,
}

#[repr(C)]
struct ExportBuffer {
    kind: u32,
    index: u32,
    plane: u32,
    flags: u32,
    fd: i32,
    reserved: [u32; 11],
}

#[repr(C)]
struct Selection {
    kind: u32,
    target: u32,
    flags: u32,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    reserved: [u32; 9],
}

#[repr(C)]
struct StreamParm {
    kind: u32,
    capability: u32,
    outputmode: u32,
    numerator: u32,
    denominator: u32,
    extendedmode: u32,
    writebuffers: u32,
    reserved: [u32; 44],
}

#[repr(C)]
struct Control {
    id: u32,
    value: i32,
}

#[repr(C)]
struct EncoderCmd {
    cmd: u32,
    flags: u32,
    raw: [u32; 8],
}

const _: () = {
    assert!(size_of::<Capability>() == 104);
    assert!(size_of::<FmtDesc>() == 64);
    assert!(size_of::<PixFormatMplane>() == 192);
    assert!(size_of::<Format>() == 208);
    assert!(size_of::<RequestBuffers>() == 20);
    assert!(size_of::<Plane>() == 64);
    assert!(size_of::<Buffer>() == 88);
    assert!(size_of::<ExportBuffer>() == 64);
    assert!(size_of::<Selection>() == 64);
    assert!(size_of::<StreamParm>() == 204);
    assert!(size_of::<EncoderCmd>() == 40);
};

const fn request<T>(write: bool, number: u32) -> libc::c_ulong {
    let direction = if write { 3 } else { 2 };
    ((direction << 30) | ((size_of::<T>() as u32) << 16) | ((b'V' as u32) << 8) | number)
        as libc::c_ulong
}

const QUERYCAP: libc::c_ulong = request::<Capability>(false, 0);
const ENUM_FMT: libc::c_ulong = request::<FmtDesc>(true, 2);
const S_FMT: libc::c_ulong = request::<Format>(true, 5);
const REQBUFS: libc::c_ulong = request::<RequestBuffers>(true, 8);
const QUERYBUF: libc::c_ulong = request::<Buffer>(true, 9);
const QBUF: libc::c_ulong = request::<Buffer>(true, 15);
const EXPBUF: libc::c_ulong = request::<ExportBuffer>(true, 16);
const DQBUF: libc::c_ulong = request::<Buffer>(true, 17);
/// _IOW: write only.
const STREAMON: libc::c_ulong = (1 << 30) | (4 << 16) | ((b'V' as libc::c_ulong) << 8) | 18;
const S_PARM: libc::c_ulong = request::<StreamParm>(true, 22);
const S_CTRL: libc::c_ulong = request::<Control>(true, 28);
const ENCODER_CMD: libc::c_ulong = request::<EncoderCmd>(true, 77);
const S_SELECTION: libc::c_ulong = request::<Selection>(true, 95);

fn zeroed<T>() -> T {
    // SAFETY: only used for the plain-data V4L2 structs above, for which all
    // zero bytes are valid.
    unsafe { std::mem::zeroed() }
}

fn ioctl<T>(file: &File, request: libc::c_ulong, argument: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: every request constant is built from the size of the struct
        // it is called with.
        if unsafe { libc::ioctl(file.as_raw_fd(), request as _, argument as *mut T) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn formats(file: &File, kind: u32) -> Vec<u32> {
    let mut formats = Vec::new();
    for index in 0.. {
        let mut desc = FmtDesc {
            index,
            kind,
            ..zeroed()
        };
        if ioctl(file, ENUM_FMT, &mut desc).is_err() {
            break;
        }
        formats.push(desc.pixelformat);
    }
    formats
}

/// The first video node that encodes NV12 into H.264. Nodes in a media
/// graph are skipped unopened: they are camera pipelines, and opening one
/// powers it up.
pub fn find_h264_encoder() -> Option<PathBuf> {
    let mut nodes: Vec<String> = fs::read_dir("/sys/class/video4linux")
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("video"))
        .collect();
    nodes.sort_by_key(|name| name[5..].parse::<u32>().unwrap_or(u32::MAX));
    nodes
        .into_iter()
        .filter(|name| !in_media_graph(name))
        .map(|name| Path::new("/dev").join(name))
        .find(|path| is_h264_encoder(path))
}

fn in_media_graph(node: &str) -> bool {
    fs::read_dir(
        Path::new("/sys/class/video4linux")
            .join(node)
            .join("device"),
    )
    .is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with("media"))
    })
}

fn is_h264_encoder(path: &Path) -> bool {
    let Ok(file) = File::options().read(true).write(true).open(path) else {
        return false;
    };
    let mut capability: Capability = zeroed();
    if ioctl(&file, QUERYCAP, &mut capability).is_err() {
        return false;
    }
    let caps = if capability.capabilities & CAP_DEVICE_CAPS != 0 {
        capability.device_caps
    } else {
        capability.capabilities
    };
    caps & CAP_VIDEO_M2M_MPLANE != 0
        && formats(&file, CAPTURE_MPLANE).contains(&H264)
        && formats(&file, OUTPUT_MPLANE).contains(&NV12)
}

/// Where the planes of every input buffer lie. Both planes share `stride`.
#[derive(Clone, Copy)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub chroma_offset: u32,
}

/// One bitstream buffer, mapped for reading.
struct Mapping {
    pointer: *mut libc::c_void,
    length: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: the mapping was made with this pointer and length.
        unsafe { libc::munmap(self.pointer, self.length) };
    }
}

pub struct Encoder {
    file: File,
    input_size: u32,
    queued: Vec<bool>,
    outputs: Vec<Mapping>,
    frames: u64,
    period: Duration,
    last: bool,
}

// SAFETY: the mappings are only read, by whichever thread owns the encoder.
unsafe impl Send for Encoder {}

impl Encoder {
    /// Opens `path` for `width`x`height` frames at `fps`, with the input
    /// buffers' DMA-BUFs and their layout.
    pub fn open(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    ) -> io::Result<(Self, Vec<OwnedFd>, Layout)> {
        let file = File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;

        // The coded format first, then the raw one, then the visible size:
        // the raw format's size is the device's aligned buffer size.
        let mut coded = Format {
            kind: CAPTURE_MPLANE,
            ..zeroed()
        };
        coded.fmt.pix_mp = PixFormatMplane {
            width,
            height,
            pixelformat: H264,
            field: FIELD_NONE,
            num_planes: 1,
            ..zeroed()
        };
        ioctl(&file, S_FMT, &mut coded)?;
        let mut raw = Format {
            kind: OUTPUT_MPLANE,
            ..zeroed()
        };
        raw.fmt.pix_mp = PixFormatMplane {
            width,
            height,
            pixelformat: NV12,
            field: FIELD_NONE,
            colorspace: COLORSPACE_REC709,
            num_planes: 1,
            ycbcr_enc: YCBCR_ENC_709,
            quantization: QUANTIZATION_LIM_RANGE,
            xfer_func: XFER_FUNC_709,
            ..zeroed()
        };
        ioctl(&file, S_FMT, &mut raw)?;
        // SAFETY: S_FMT on a multi-planar type fills pix_mp.
        let raw = unsafe { raw.fmt.pix_mp };
        if raw.pixelformat != NV12 || raw.num_planes != 1 {
            return Err(io::Error::other("the encoder refused NV12 input"));
        }
        let layout = Layout {
            width,
            height,
            stride: raw.plane_fmt[0].bytesperline,
            chroma_offset: raw.plane_fmt[0].bytesperline * raw.height,
        };
        ioctl(
            &file,
            S_SELECTION,
            &mut Selection {
                kind: OUTPUT,
                target: SEL_TGT_CROP,
                width,
                height,
                ..zeroed()
            },
        )?;
        // The rate frames arrive at, and the coded rate rate control
        // budgets the bitrate by.
        for kind in [OUTPUT_MPLANE, CAPTURE_MPLANE] {
            ioctl(
                &file,
                S_PARM,
                &mut StreamParm {
                    kind,
                    numerator: 1,
                    denominator: fps,
                    ..zeroed()
                },
            )?;
        }
        for (id, value) in [
            (CID_H264_PROFILE, H264_PROFILE_HIGH),
            (CID_H264_LEVEL, H264_LEVEL_5_1),
            (CID_BITRATE_MODE, BITRATE_MODE_VBR),
            (CID_BITRATE, bitrate as i32),
            (CID_GOP_SIZE, fps as i32),
            (CID_B_FRAMES, 0),
        ] {
            ioctl(&file, S_CTRL, &mut Control { id, value })
                .map_err(|error| io::Error::other(format!("control {id:#x}: {error}")))?;
        }

        let inputs = request_buffers(&file, OUTPUT_MPLANE, INPUTS)?;
        let dmabufs = (0..inputs)
            .map(|index| {
                let mut export = ExportBuffer {
                    kind: OUTPUT_MPLANE,
                    index,
                    flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
                    ..zeroed()
                };
                ioctl(&file, EXPBUF, &mut export)?;
                // SAFETY: EXPBUF returned a new descriptor this process owns.
                Ok(unsafe { OwnedFd::from_raw_fd(export.fd) })
            })
            .collect::<io::Result<Vec<_>>>()?;

        let mut encoder = Self {
            file,
            input_size: raw.plane_fmt[0].sizeimage,
            queued: vec![false; inputs as usize],
            outputs: Vec::new(),
            frames: 0,
            period: Duration::from_secs(1) / fps,
            last: false,
        };
        for index in 0..request_buffers(&encoder.file, CAPTURE_MPLANE, OUTPUTS)? {
            let mut plane: Plane = zeroed();
            let mut buffer = Buffer {
                index,
                kind: CAPTURE_MPLANE,
                memory: MEMORY_MMAP,
                planes: &mut plane,
                length: 1,
                ..zeroed()
            };
            ioctl(&encoder.file, QUERYBUF, &mut buffer)?;
            let length = plane.length as usize;
            // SAFETY: maps the buffer QUERYBUF described, read only.
            let pointer = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    encoder.file.as_raw_fd(),
                    (plane.m & 0xffff_ffff) as libc::off_t,
                )
            };
            if pointer == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            encoder.outputs.push(Mapping { pointer, length });
            encoder.queue_output(index)?;
        }
        for kind in [OUTPUT_MPLANE, CAPTURE_MPLANE] {
            let mut kind = kind as i32;
            ioctl(&encoder.file, STREAMON, &mut kind)?;
        }
        Ok((encoder, dmabufs, layout))
    }

    /// Whether the device still holds input `index`.
    pub fn holds(&self, index: usize) -> bool {
        self.queued[index]
    }

    /// Waits until the device releases input `index`. The stream that comes
    /// out meanwhile goes to `out`, and every input the device releases goes
    /// to `released`.
    pub fn reclaim(
        &mut self,
        index: usize,
        out: &mut impl Write,
        released: &mut impl FnMut(usize),
    ) -> io::Result<()> {
        while self.queued[index] {
            self.pump(true, out, released)?;
        }
        Ok(())
    }

    /// Encodes input `index` once the device has released it, as `reclaim`.
    pub fn encode(
        &mut self,
        index: usize,
        out: &mut impl Write,
        released: &mut impl FnMut(usize),
    ) -> io::Result<()> {
        self.reclaim(index, out, released)?;
        let mut plane = Plane {
            bytesused: self.input_size,
            length: self.input_size,
            ..zeroed()
        };
        let time = self.period * self.frames as u32;
        let mut buffer = Buffer {
            index: index as u32,
            kind: OUTPUT_MPLANE,
            field: FIELD_NONE,
            timestamp: libc::timeval {
                tv_sec: time.as_secs() as libc::time_t,
                tv_usec: time.subsec_micros() as libc::suseconds_t,
            },
            memory: MEMORY_MMAP,
            planes: &mut plane,
            length: 1,
            ..zeroed()
        };
        ioctl(&self.file, QBUF, &mut buffer)?;
        self.queued[index] = true;
        self.frames += 1;
        self.pump(false, out, released)
    }

    /// Encodes everything queued and writes the rest of the stream.
    pub fn finish(
        mut self,
        out: &mut impl Write,
        released: &mut impl FnMut(usize),
    ) -> io::Result<()> {
        if self.frames == 0 {
            return Ok(());
        }
        ioctl(
            &self.file,
            ENCODER_CMD,
            &mut EncoderCmd {
                cmd: ENC_CMD_STOP,
                ..zeroed()
            },
        )?;
        while !self.last {
            self.pump(true, out, released)?;
        }
        Ok(())
    }

    fn queue_output(&self, index: u32) -> io::Result<()> {
        let mut plane = Plane {
            length: self.outputs[index as usize].length as u32,
            ..zeroed()
        };
        let mut buffer = Buffer {
            index,
            kind: CAPTURE_MPLANE,
            memory: MEMORY_MMAP,
            planes: &mut plane,
            length: 1,
            ..zeroed()
        };
        ioctl(&self.file, QBUF, &mut buffer)
    }

    /// Dequeues what the device has finished: stream buffers into `out`,
    /// inputs into `released`. With `wait`, waits for at least one.
    fn pump(
        &mut self,
        wait: bool,
        out: &mut impl Write,
        released: &mut impl FnMut(usize),
    ) -> io::Result<()> {
        if wait {
            let mut poll = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN | libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: `poll` is one valid pollfd.
            match unsafe { libc::poll(&mut poll, 1, STALL.as_millis() as i32) } {
                0 => return Err(io::Error::other("the hardware encoder stalled")),
                -1 => return Err(io::Error::last_os_error()),
                _ if poll.revents & libc::POLLERR != 0 => {
                    return Err(io::Error::other("the hardware encoder failed"));
                }
                _ => {}
            }
        }
        loop {
            let mut plane: Plane = zeroed();
            let mut buffer = Buffer {
                kind: CAPTURE_MPLANE,
                memory: MEMORY_MMAP,
                planes: &mut plane,
                length: 1,
                ..zeroed()
            };
            match ioctl(&self.file, DQBUF, &mut buffer) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                // After the last buffer, DQBUF reports EPIPE.
                Err(_) if self.last => break,
                Err(error) => return Err(error),
            }
            let mapping = &self.outputs[buffer.index as usize];
            let start = plane.data_offset as usize;
            let end = (plane.bytesused as usize).min(mapping.length);
            if end > start {
                // SAFETY: the device filled bytes start..end of this mapping
                // and holds no claim on it until it is queued again.
                out.write_all(unsafe {
                    std::slice::from_raw_parts(mapping.pointer.cast::<u8>().add(start), end - start)
                })?;
            }
            if buffer.flags & BUF_FLAG_LAST != 0 {
                self.last = true;
                break;
            }
            self.queue_output(buffer.index)?;
        }
        loop {
            let mut plane: Plane = zeroed();
            let mut buffer = Buffer {
                kind: OUTPUT_MPLANE,
                memory: MEMORY_MMAP,
                planes: &mut plane,
                length: 1,
                ..zeroed()
            };
            match ioctl(&self.file, DQBUF, &mut buffer) {
                Ok(()) => {
                    self.queued[buffer.index as usize] = false;
                    released(buffer.index as usize);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) if self.last => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn request_buffers(file: &File, kind: u32, count: u32) -> io::Result<u32> {
    let mut request = RequestBuffers {
        count,
        kind,
        memory: MEMORY_MMAP,
        ..zeroed()
    };
    ioctl(file, REQBUFS, &mut request)?;
    if request.count == 0 {
        return Err(io::Error::other("the encoder gave no buffers"));
    }
    Ok(request.count)
}
