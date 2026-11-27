//! A stateful V4L2 memory-to-memory decoder (the kernel's "dev-decoder"
//! interface), such as Qualcomm Iris: compressed access units go in on the
//! OUTPUT queue, NV12 frames come out on the CAPTURE queue. Each CAPTURE
//! buffer is exported once as a DMA-BUF, so a frame reaches the compositor
//! as the decoder wrote it.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

const TYPE_CAPTURE: u32 = 1;
const TYPE_CAPTURE_MPLANE: u32 = 9;
const TYPE_OUTPUT_MPLANE: u32 = 10;
const MEMORY_MMAP: u32 = 1;
const CAP_VIDEO_M2M_MPLANE: u32 = 0x4000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const EVENT_EOS: u32 = 2;
const EVENT_SOURCE_CHANGE: u32 = 5;
const BUF_FLAG_ERROR: u32 = 0x40;
const BUF_FLAG_LAST: u32 = 0x10_0000;
const SEL_TGT_COMPOSE: u32 = 0x100;
const CID_MIN_BUFFERS_FOR_CAPTURE: u32 = 0x0098_0927;

pub const PIX_FMT_H264: u32 = u32::from_le_bytes(*b"H264");
pub const PIX_FMT_HEVC: u32 = u32::from_le_bytes(*b"HEVC");
pub const PIX_FMT_NV12: u32 = u32::from_le_bytes(*b"NV12");

/// CAPTURE buffers beyond the decoder's minimum: one on screen, one the
/// compositor may still read, and frames decoded ahead.
const SPARE_CAPTURE: u32 = 4;
const OUTPUT_BUFFERS: u32 = 4;

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
struct PlanePixFormat {
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
    plane_fmt: [PlanePixFormat; 8],
    num_planes: u8,
    flags: u8,
    ycbcr_enc: u8,
    quantization: u8,
    xfer_func: u8,
    reserved: [u8; 7],
}

#[repr(C)]
union FormatUnion {
    pix_mp: PixFormatMplane,
    raw: [u64; 25],
}

#[repr(C)]
struct Format {
    kind: u32,
    fmt: FormatUnion,
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
#[derive(Clone, Copy)]
struct Plane {
    bytesused: u32,
    length: u32,
    /// mem_offset for MMAP.
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
    timestamp: [i64; 2],
    timecode: [u32; 4],
    sequence: u32,
    memory: u32,
    /// The planes array for multi-planar buffers.
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
struct EventSubscription {
    kind: u32,
    id: u32,
    flags: u32,
    reserved: [u32; 5],
}

#[repr(C)]
struct Event {
    kind: u32,
    u: [u64; 8],
    pending: u32,
    sequence: u32,
    timestamp: [i64; 2],
    id: u32,
    reserved: [u32; 8],
}

#[repr(C)]
struct Selection {
    kind: u32,
    target: u32,
    flags: u32,
    r: [i32; 4],
    reserved: [u32; 9],
}

#[repr(C)]
struct Control {
    id: u32,
    value: i32,
}

const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | nr
}
const R: u64 = 2;
const W: u64 = 1;
const RW: u64 = 3;
const QUERYCAP: u64 = ioc(R, 0, size_of::<Capability>());
const ENUM_FMT: u64 = ioc(RW, 2, size_of::<FmtDesc>());
const G_FMT: u64 = ioc(RW, 4, size_of::<Format>());
const S_FMT: u64 = ioc(RW, 5, size_of::<Format>());
const REQBUFS: u64 = ioc(RW, 8, size_of::<RequestBuffers>());
const QUERYBUF: u64 = ioc(RW, 9, size_of::<Buffer>());
const QBUF: u64 = ioc(RW, 15, size_of::<Buffer>());
const EXPBUF: u64 = ioc(RW, 16, size_of::<ExportBuffer>());
const DQBUF: u64 = ioc(RW, 17, size_of::<Buffer>());
const STREAMON: u64 = ioc(W, 18, size_of::<i32>());
const G_CTRL: u64 = ioc(RW, 27, size_of::<Control>());
const DQEVENT: u64 = ioc(R, 89, size_of::<Event>());
const SUBSCRIBE_EVENT: u64 = ioc(W, 90, size_of::<EventSubscription>());
const G_SELECTION: u64 = ioc(RW, 94, size_of::<Selection>());

const _: () = {
    assert!(size_of::<Format>() == 208);
    assert!(size_of::<Buffer>() == 88);
    assert!(size_of::<Event>() == 136);
    assert!(QBUF == 0xc058_560f);
    assert!(DQEVENT == 0x8088_5659);
    assert!(G_SELECTION == 0xc040_565e);
};

fn zeroed<T>() -> T {
    // SAFETY: every type used here is plain data for which zero is valid.
    unsafe { std::mem::zeroed() }
}

fn ioctl<T>(fd: RawFd, request: u64, arg: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: `arg` is the structure `request` names, sized into it.
        if unsafe { libc::ioctl(fd, request as _, arg as *mut T) } >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn context(error: io::Error, what: &str) -> io::Error {
    io::Error::new(error.kind(), format!("{what}: {error}"))
}

struct Mapping {
    pointer: *mut u8,
    length: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: mapped by us with this length.
        unsafe { libc::munmap(self.pointer.cast(), self.length) };
    }
}

/// How the decoded frames lie in their buffers.
#[derive(Clone, Copy, Debug)]
pub struct FrameLayout {
    pub stride: u32,
    /// Where the interleaved chroma plane starts.
    pub chroma_offset: u32,
    /// The picture's rectangle in the buffer: x, y, width, height.
    pub visible: [u32; 4],
}

pub struct Decoder {
    fd: OwnedFd,
    pub node: PathBuf,
    bitstream: Vec<Mapping>,
    free_bitstream: Vec<u32>,
    queued_bitstream: usize,
    /// One exported DMA-BUF per CAPTURE buffer, by index.
    pub frames: Vec<OwnedFd>,
    queued_frames: usize,
    pub layout: Option<FrameLayout>,
}

fn formats(fd: RawFd, kind: u32) -> Vec<u32> {
    (0..)
        .map_while(|index| {
            let mut desc: FmtDesc = zeroed();
            desc.index = index;
            desc.kind = kind;
            ioctl(fd, ENUM_FMT, &mut desc)
                .ok()
                .map(|_| desc.pixelformat)
        })
        .collect()
}

fn open_node(node: &Path) -> io::Result<OwnedFd> {
    let path = CString::new(node.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid C string.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: just opened.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The first memory-to-memory decoder that takes `codec` and gives NV12.
fn find_decoder(codec: u32) -> io::Result<(PathBuf, OwnedFd)> {
    let mut nodes = fs::read_dir("/sys/class/video4linux")?
        .filter_map(|entry| entry.ok())
        .map(|entry| Path::new("/dev").join(entry.file_name()))
        .collect::<Vec<_>>();
    nodes.sort();
    for node in nodes {
        let Ok(fd) = open_node(&node) else {
            continue;
        };
        let mut cap: Capability = zeroed();
        if ioctl(fd.as_raw_fd(), QUERYCAP, &mut cap).is_err() {
            continue;
        }
        let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        if caps & CAP_VIDEO_M2M_MPLANE != 0
            && formats(fd.as_raw_fd(), TYPE_OUTPUT_MPLANE).contains(&codec)
            && formats(fd.as_raw_fd(), TYPE_CAPTURE_MPLANE).contains(&PIX_FMT_NV12)
        {
            return Ok((node, fd));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no hardware decoder takes this video",
    ))
}

/// What the decoder said, besides frames.
pub enum Notice {
    /// The CAPTURE format is known or changed: frames come once
    /// `setup_frames` ran.
    SourceChange,
    EndOfStream,
}

impl Decoder {
    /// Opens the decoder for `codec` at the coded size, with bitstream
    /// buffers of at least `bitstream_size` bytes, and starts its OUTPUT queue.
    pub fn open(codec: u32, width: u32, height: u32, bitstream_size: usize) -> io::Result<Self> {
        let (node, fd) = find_decoder(codec)?;
        let raw = fd.as_raw_fd();

        let mut format: Format = zeroed();
        format.kind = TYPE_OUTPUT_MPLANE;
        // SAFETY: the multi-planar member of a zeroed union.
        let pix = unsafe { &mut format.fmt.pix_mp };
        pix.width = width;
        pix.height = height;
        pix.pixelformat = codec;
        pix.num_planes = 1;
        pix.plane_fmt[0].sizeimage = bitstream_size as u32;
        ioctl(raw, S_FMT, &mut format).map_err(|e| context(e, "set the bitstream format"))?;

        for kind in [EVENT_SOURCE_CHANGE, EVENT_EOS] {
            let mut subscription: EventSubscription = zeroed();
            subscription.kind = kind;
            ioctl(raw, SUBSCRIBE_EVENT, &mut subscription)
                .map_err(|e| context(e, "subscribe to decoder events"))?;
        }

        let mut request: RequestBuffers = zeroed();
        request.count = OUTPUT_BUFFERS;
        request.kind = TYPE_OUTPUT_MPLANE;
        request.memory = MEMORY_MMAP;
        ioctl(raw, REQBUFS, &mut request).map_err(|e| context(e, "allocate bitstream buffers"))?;
        let mut bitstream = Vec::new();
        for index in 0..request.count {
            let mut planes = [zeroed::<Plane>(); 1];
            let mut buffer: Buffer = zeroed();
            buffer.index = index;
            buffer.kind = TYPE_OUTPUT_MPLANE;
            buffer.memory = MEMORY_MMAP;
            buffer.planes = planes.as_mut_ptr();
            buffer.length = 1;
            ioctl(raw, QUERYBUF, &mut buffer)
                .map_err(|e| context(e, "query a bitstream buffer"))?;
            let length = planes[0].length as usize;
            // SAFETY: maps the buffer the driver just described.
            let pointer = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    raw,
                    planes[0].m as libc::off_t,
                )
            };
            if pointer == libc::MAP_FAILED {
                return Err(context(
                    io::Error::last_os_error(),
                    "map a bitstream buffer",
                ));
            }
            bitstream.push(Mapping {
                pointer: pointer.cast(),
                length,
            });
        }
        let mut kind = TYPE_OUTPUT_MPLANE as i32;
        ioctl(raw, STREAMON, &mut kind).map_err(|e| context(e, "start the bitstream queue"))?;

        Ok(Self {
            fd,
            node,
            free_bitstream: (0..bitstream.len() as u32).rev().collect(),
            bitstream,
            queued_bitstream: 0,
            frames: Vec::new(),
            queued_frames: 0,
            layout: None,
        })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Whether either queue holds buffers, so polling the decoder can wake.
    pub fn busy(&self) -> bool {
        self.queued_bitstream > 0 || self.queued_frames > 0
    }

    pub fn has_free_bitstream(&self) -> bool {
        !self.free_bitstream.is_empty()
    }

    /// Fills a free bitstream buffer with `fill`, which returns the bytes it
    /// wrote, and queues it.
    pub fn queue_bitstream(
        &mut self,
        fill: impl FnOnce(&mut [u8]) -> io::Result<usize>,
    ) -> io::Result<()> {
        let index = *self.free_bitstream.last().expect("a free bitstream buffer");
        let mapping = &self.bitstream[index as usize];
        // SAFETY: the mapping lives as long as self and the driver does not
        // touch a buffer that is not queued.
        let memory = unsafe { std::slice::from_raw_parts_mut(mapping.pointer, mapping.length) };
        let used = fill(memory)?;
        let mut planes = [zeroed::<Plane>(); 1];
        planes[0].bytesused = used as u32;
        planes[0].length = mapping.length as u32;
        let mut buffer: Buffer = zeroed();
        buffer.index = index;
        buffer.kind = TYPE_OUTPUT_MPLANE;
        buffer.memory = MEMORY_MMAP;
        buffer.planes = planes.as_mut_ptr();
        buffer.length = 1;
        ioctl(self.raw_fd(), QBUF, &mut buffer).map_err(|e| context(e, "queue bitstream"))?;
        self.free_bitstream.pop();
        self.queued_bitstream += 1;
        Ok(())
    }

    fn dequeue(&mut self, kind: u32) -> io::Result<Option<(u32, u32, u32)>> {
        let mut planes = [zeroed::<Plane>(); 1];
        let mut buffer: Buffer = zeroed();
        buffer.kind = kind;
        buffer.memory = MEMORY_MMAP;
        buffer.planes = planes.as_mut_ptr();
        buffer.length = 1;
        match ioctl(self.raw_fd(), DQBUF, &mut buffer) {
            Ok(()) => Ok(Some((buffer.index, buffer.flags, planes[0].bytesused))),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(libc::EPIPE) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Takes back the bitstream buffers the decoder has read.
    pub fn reclaim_bitstream(&mut self) -> io::Result<()> {
        while let Some((index, _, _)) = self.dequeue(TYPE_OUTPUT_MPLANE)? {
            self.queued_bitstream -= 1;
            self.free_bitstream.push(index);
        }
        Ok(())
    }

    /// The next decoded frame's buffer index, if one is done. Empty and
    /// failed buffers go straight back to the decoder.
    pub fn next_frame(&mut self) -> io::Result<Option<u32>> {
        if self.frames.is_empty() {
            return Ok(None);
        }
        while let Some((index, flags, bytesused)) = self.dequeue(TYPE_CAPTURE_MPLANE)? {
            self.queued_frames -= 1;
            if bytesused == 0 || flags & BUF_FLAG_ERROR != 0 {
                if flags & BUF_FLAG_LAST == 0 {
                    self.queue_frame(index)?;
                }
                continue;
            }
            return Ok(Some(index));
        }
        Ok(None)
    }

    /// Gives a frame's buffer back to the decoder to write into.
    pub fn queue_frame(&mut self, index: u32) -> io::Result<()> {
        let mut planes = [zeroed::<Plane>(); 1];
        let mut buffer: Buffer = zeroed();
        buffer.index = index;
        buffer.kind = TYPE_CAPTURE_MPLANE;
        buffer.memory = MEMORY_MMAP;
        buffer.planes = planes.as_mut_ptr();
        buffer.length = 1;
        ioctl(self.raw_fd(), QBUF, &mut buffer).map_err(|e| context(e, "queue a frame buffer"))?;
        self.queued_frames += 1;
        Ok(())
    }

    /// The next pending event.
    pub fn next_notice(&mut self) -> io::Result<Option<Notice>> {
        loop {
            let mut event: Event = zeroed();
            match ioctl(self.raw_fd(), DQEVENT, &mut event) {
                Ok(()) => match event.kind {
                    EVENT_SOURCE_CHANGE => return Ok(Some(Notice::SourceChange)),
                    EVENT_EOS => return Ok(Some(Notice::EndOfStream)),
                    _ => continue,
                },
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }

    /// Sets up NV12 frame buffers for the format the decoder found, exports
    /// each once and queues them all. Only for the first format: a later
    /// change of size is not handled.
    pub fn setup_frames(&mut self) -> io::Result<FrameLayout> {
        if let Some(layout) = self.layout {
            return Ok(layout);
        }
        let raw = self.raw_fd();
        let mut format: Format = zeroed();
        format.kind = TYPE_CAPTURE_MPLANE;
        ioctl(raw, G_FMT, &mut format).map_err(|e| context(e, "read the frame format"))?;
        // SAFETY: the driver filled the multi-planar member.
        if unsafe { format.fmt.pix_mp.pixelformat } != PIX_FMT_NV12 {
            format.fmt.pix_mp.pixelformat = PIX_FMT_NV12;
            ioctl(raw, S_FMT, &mut format).map_err(|e| context(e, "choose NV12 frames"))?;
        }
        // SAFETY: as above.
        let pix = unsafe { format.fmt.pix_mp };
        if pix.pixelformat != PIX_FMT_NV12 || pix.num_planes != 1 {
            return Err(io::Error::other("the decoder gives no single-buffer NV12"));
        }
        let stride = pix.plane_fmt[0].bytesperline;

        let mut selection: Selection = zeroed();
        selection.kind = TYPE_CAPTURE;
        selection.target = SEL_TGT_COMPOSE;
        let visible = match ioctl(raw, G_SELECTION, &mut selection) {
            Ok(()) => selection.r.map(|value| value.max(0) as u32),
            Err(_) => [0, 0, pix.width, pix.height],
        };

        let mut minimum = Control {
            id: CID_MIN_BUFFERS_FOR_CAPTURE,
            value: 0,
        };
        let minimum = match ioctl(raw, G_CTRL, &mut minimum) {
            Ok(()) => minimum.value.max(1) as u32,
            Err(_) => 4,
        };

        let mut request: RequestBuffers = zeroed();
        request.count = minimum + SPARE_CAPTURE;
        request.kind = TYPE_CAPTURE_MPLANE;
        request.memory = MEMORY_MMAP;
        ioctl(raw, REQBUFS, &mut request).map_err(|e| context(e, "allocate frame buffers"))?;
        for index in 0..request.count {
            let mut export: ExportBuffer = zeroed();
            export.kind = TYPE_CAPTURE_MPLANE;
            export.index = index;
            export.flags = (libc::O_CLOEXEC | libc::O_RDWR) as u32;
            ioctl(raw, EXPBUF, &mut export).map_err(|e| context(e, "export a frame buffer"))?;
            // SAFETY: the driver just made this descriptor for us.
            self.frames.push(unsafe { OwnedFd::from_raw_fd(export.fd) });
        }
        for index in 0..request.count {
            self.queue_frame(index)?;
        }
        let mut kind = TYPE_CAPTURE_MPLANE as i32;
        ioctl(raw, STREAMON, &mut kind).map_err(|e| context(e, "start the frame queue"))?;

        let layout = FrameLayout {
            stride,
            // The chroma plane follows the luma plane's full height, as
            // FFmpeg and GStreamer read these buffers.
            chroma_offset: stride * pix.height,
            visible,
        };
        eprintln!(
            "video-wallpaper: {} decodes {}x{} frames (stride {}, {} buffers, minimum {})",
            self.node.display(),
            visible[2],
            visible[3],
            stride,
            request.count,
            minimum
        );
        self.layout = Some(layout);
        Ok(layout)
    }
}
