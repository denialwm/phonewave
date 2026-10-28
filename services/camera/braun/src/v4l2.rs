//! The V4L2 ioctls braun uses, with the kernel's structs laid out as the
//! kernel lays them: subdevice formats, controls, and the capture side of a
//! video node.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

pub const fn iowr(kind: u8, nr: u8, size: usize) -> libc::c_ulong {
    (3 << 30)
        | ((size as libc::c_ulong) << 16)
        | ((kind as libc::c_ulong) << 8)
        | nr as libc::c_ulong
}

pub const fn iow(kind: u8, nr: u8, size: usize) -> libc::c_ulong {
    (1 << 30)
        | ((size as libc::c_ulong) << 16)
        | ((kind as libc::c_ulong) << 8)
        | nr as libc::c_ulong
}

pub fn ioctl<T>(file: &File, request: libc::c_ulong, arg: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: `arg` is the request's argument struct, laid out as the
        // kernel's, and lives across the call.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), request, arg as *mut T) };
        if result >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

pub fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MbusFormat {
    pub width: u32,
    pub height: u32,
    pub code: u32,
    pub field: u32,
    pub colorspace: u32,
    pub ycbcr_enc: u16,
    pub quantization: u16,
    pub xfer_func: u16,
    pub flags: u16,
    pub reserved: [u16; 10],
}

#[repr(C)]
#[derive(Default)]
struct SubdevFormat {
    which: u32,
    pad: u32,
    format: MbusFormat,
    stream: u32,
    reserved: [u32; 7],
}

const VIDIOC_SUBDEV_G_FMT: libc::c_ulong = iowr(b'V', 4, size_of::<SubdevFormat>());
const VIDIOC_SUBDEV_S_FMT: libc::c_ulong = iowr(b'V', 5, size_of::<SubdevFormat>());
const SUBDEV_FORMAT_ACTIVE: u32 = 1;
pub const FIELD_NONE: u32 = 1;

/// The active format on `pad` of a subdevice.
pub fn subdev_format(subdev: &File, pad: u32) -> io::Result<MbusFormat> {
    let mut format = SubdevFormat {
        which: SUBDEV_FORMAT_ACTIVE,
        pad,
        ..Default::default()
    };
    ioctl(subdev, VIDIOC_SUBDEV_G_FMT, &mut format)?;
    Ok(format.format)
}

pub fn set_subdev_format(
    subdev: &File,
    pad: u32,
    width: u32,
    height: u32,
    code: u32,
) -> io::Result<()> {
    let mut format = SubdevFormat {
        which: SUBDEV_FORMAT_ACTIVE,
        pad,
        format: MbusFormat {
            width,
            height,
            code,
            field: FIELD_NONE,
            ..Default::default()
        },
        ..Default::default()
    };
    ioctl(subdev, VIDIOC_SUBDEV_S_FMT, &mut format)
}

/// Control IDs.
pub mod cid {
    pub const EXPOSURE: u32 = 0x0098_0911;
    pub const FOCUS_ABSOLUTE: u32 = 0x009a_090a;
    pub const CAMERA_ORIENTATION: u32 = 0x009a_0922;
    pub const CAMERA_SENSOR_ROTATION: u32 = 0x009a_0923;
    pub const HBLANK: u32 = 0x009e_0902;
    pub const ANALOGUE_GAIN: u32 = 0x009e_0903;
    pub const PIXEL_RATE: u32 = 0x009f_0902;
}

#[repr(C)]
struct QueryExtCtrl {
    id: u32,
    kind: u32,
    name: [u8; 32],
    minimum: i64,
    maximum: i64,
    step: u64,
    default_value: i64,
    flags: u32,
    elem_size: u32,
    elems: u32,
    nr_of_dims: u32,
    dims: [u32; 4],
    reserved: [u32; 32],
}

#[repr(C, packed)]
struct ExtControl {
    id: u32,
    size: u32,
    reserved2: u32,
    value: [u8; 8],
}

#[repr(C)]
struct ExtControls {
    which: u32,
    count: u32,
    error_idx: u32,
    request_fd: i32,
    reserved: u32,
    controls: *mut ExtControl,
}

#[repr(C)]
struct Control {
    id: u32,
    value: i32,
}

const VIDIOC_S_CTRL: libc::c_ulong = iowr(b'V', 28, size_of::<Control>());
const VIDIOC_G_EXT_CTRLS: libc::c_ulong = iowr(b'V', 71, size_of::<ExtControls>());
const VIDIOC_QUERY_EXT_CTRL: libc::c_ulong = iowr(b'V', 103, size_of::<QueryExtCtrl>());
const CTRL_TYPE_INTEGER64: u32 = 5;
const CTRL_WHICH_CUR_VAL: u32 = 0;

/// What values a control takes.
#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub min: i64,
    pub max: i64,
    pub step: i64,
}

fn query(file: &File, id: u32) -> io::Result<QueryExtCtrl> {
    // SAFETY: all-zero is a valid QueryExtCtrl.
    let mut query: QueryExtCtrl = unsafe { std::mem::zeroed() };
    query.id = id;
    ioctl(file, VIDIOC_QUERY_EXT_CTRL, &mut query)?;
    Ok(query)
}

/// The range of control `id`; an error if the device has no such control.
pub fn range(file: &File, id: u32) -> io::Result<Range> {
    let query = query(file, id)?;
    Ok(Range {
        min: query.minimum,
        max: query.maximum,
        step: query.step.max(1) as i64,
    })
}

/// The current value of control `id`.
pub fn value(file: &File, id: u32) -> io::Result<i64> {
    let wide = query(file, id)?.kind == CTRL_TYPE_INTEGER64;
    let mut control = ExtControl {
        id,
        size: 0,
        reserved2: 0,
        value: [0; 8],
    };
    let mut controls = ExtControls {
        which: CTRL_WHICH_CUR_VAL,
        count: 1,
        error_idx: 0,
        request_fd: 0,
        reserved: 0,
        controls: &mut control,
    };
    ioctl(file, VIDIOC_G_EXT_CTRLS, &mut controls)?;
    let bytes = control.value;
    Ok(if wide {
        i64::from_le_bytes(bytes)
    } else {
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64
    })
}

pub fn set(file: &File, id: u32, value: i32) -> io::Result<()> {
    let mut control = Control { id, value };
    ioctl(file, VIDIOC_S_CTRL, &mut control)
}

// The capture side of a video node, multi-planar, memory-mapped.

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct PlaneFormat {
    pub sizeimage: u32,
    pub bytesperline: u32,
    pub reserved: [u16; 6],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct PixFormatMplane {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub colorspace: u32,
    pub planes: [PlaneFormat; 8],
    pub num_planes: u8,
    pub flags: u8,
    pub ycbcr_enc: u8,
    pub quantization: u8,
    pub xfer_func: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
pub struct Format {
    pub kind: u32,
    pub _pad: u32,
    pub pix: PixFormatMplane,
    // The kernel's union is 200 bytes and 8-byte aligned.
    pub _rest: [u8; 200 - std::mem::size_of::<PixFormatMplane>()],
    pub _align: [u64; 0],
}

#[repr(C)]
#[derive(Default)]
pub struct RequestBuffers {
    pub count: u32,
    pub kind: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Plane {
    pub bytesused: u32,
    pub length: u32,
    pub mem_offset: u64,
    pub data_offset: u32,
    pub reserved: [u32; 11],
}

#[repr(C)]
pub struct Buffer {
    pub index: u32,
    pub kind: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: libc::timeval,
    pub timecode: [u32; 4],
    pub sequence: u32,
    pub memory: u32,
    pub planes: *mut Plane,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}

pub const VIDIOC_S_FMT: libc::c_ulong = iowr(b'V', 5, size_of::<Format>());
pub const VIDIOC_REQBUFS: libc::c_ulong = iowr(b'V', 8, size_of::<RequestBuffers>());
pub const VIDIOC_QUERYBUF: libc::c_ulong = iowr(b'V', 9, size_of::<Buffer>());
pub const VIDIOC_QBUF: libc::c_ulong = iowr(b'V', 15, size_of::<Buffer>());
pub const VIDIOC_DQBUF: libc::c_ulong = iowr(b'V', 17, size_of::<Buffer>());
pub const VIDIOC_STREAMON: libc::c_ulong = iow(b'V', 18, size_of::<i32>());
pub const VIDIOC_STREAMOFF: libc::c_ulong = iow(b'V', 19, size_of::<i32>());

pub const BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;
pub const MEMORY_MMAP: u32 = 1;

const _: () = assert!(size_of::<SubdevFormat>() == 88);
const _: () = assert!(size_of::<QueryExtCtrl>() == 232);
const _: () = assert!(size_of::<ExtControl>() == 20);
const _: () = assert!(size_of::<ExtControls>() == 32);
const _: () = assert!(size_of::<Format>() == 208);
const _: () = assert!(size_of::<Buffer>() == 88);
const _: () = assert!(size_of::<Plane>() == 64);
