//! A camera routed to memory and streaming, through qcom-camss: the sensor's
//! CSIPHY to CSID 0 to the first raw dump interface of VFE 0, so one camera
//! streams at a time. Nothing on the way processes the frames: they arrive
//! as the sensor's packed 10-bit Bayer samples, and everything after that is
//! braun's (`isp`, `photo`).

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::sync::Arc;

use crate::camera::Camera;
use crate::media::Graph;
use crate::v4l2::{self, Buffer, Format, PixFormatMplane, Plane, RequestBuffers, cid, ioctl};

const CSID: &str = "msm_csid0";
const RDI: &str = "msm_vfe0_rdi0";
const VIDEO: &str = "msm_vfe0_video0";
/// The most CSIPHYs and CSIDs a camss SoC has; links to the ones a graph
/// lacks are off already.
const CSIPHYS: u32 = 8;
const CSIDS: u32 = 5;
/// Buffers queued to the video node.
const BUFFERS: u32 = 4;

/// One mmap'd capture buffer.
struct Mapping {
    data: *mut u8,
    length: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: mapped by `Device::open` with this length.
        unsafe { libc::munmap(self.data.cast(), self.length) };
    }
}

/// A frame the driver handed over; it goes back with the next `frame`.
pub struct Frame<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

pub struct Device {
    pub camera: Arc<Camera>,
    sensor: File,
    /// Held open while streaming: the focus motor powers down a second
    /// after its node closes.
    lens: Option<File>,
    video: File,
    buffers: Vec<Mapping>,
    stride: usize,
    /// A frame is out with the caller.
    held: Option<u32>,
}

// The mappings are only touched from the thread that owns the device.
unsafe impl Send for Device {}

impl Device {
    /// Routes `camera` to VFE 0's raw interface, sets every pad to its mode
    /// and starts streaming.
    pub fn open(camera: Arc<Camera>) -> io::Result<Self> {
        let graph = Graph::open(&camera.media)?;
        for index in 0..CSIPHYS {
            graph.link(&format!("msm_csiphy{index}"), 1, CSID, 0, false)?;
        }
        for index in 1..CSIDS {
            graph.link(&format!("msm_csid{index}"), 1, RDI, 0, false)?;
        }
        graph.link(&camera.phy, 1, CSID, 0, true)?;
        graph.link(CSID, 1, RDI, 0, true)?;

        let format = |file: &File, pad: u32| {
            v4l2::set_subdev_format(
                file,
                pad,
                camera.width,
                camera.height,
                camera.bayer.mbus_code(),
            )
        };
        let sensor = graph.node(&camera.entity)?;
        format(&sensor, 0)?;
        let phy = graph.node(&camera.phy)?;
        format(&phy, 0)?;
        format(&phy, 1)?;
        let csid = graph.node(CSID)?;
        format(&csid, 0)?;
        format(&csid, 1)?;
        let rdi = graph.node(RDI)?;
        format(&rdi, 0)?;
        format(&rdi, 1)?;
        let lens = match &camera.lens {
            Some(lens) => Some(graph.node(&lens.entity)?),
            None => None,
        };

        let video = graph.node(VIDEO)?;
        let mut format = Format {
            kind: v4l2::BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            _pad: 0,
            pix: PixFormatMplane {
                width: camera.width,
                height: camera.height,
                pixelformat: camera.bayer.fourcc(),
                field: v4l2::FIELD_NONE,
                num_planes: 1,
                ..Default::default()
            },
            _rest: [0; 200 - size_of::<PixFormatMplane>()],
            _align: [],
        };
        ioctl(&video, v4l2::VIDIOC_S_FMT, &mut format)?;
        let plane = format.pix.planes[0];
        let stride = plane.bytesperline as usize;
        if stride < camera.width as usize * 5 / 4 {
            return Err(io::Error::other("video node refused the raw format"));
        }

        let mut request = RequestBuffers {
            count: BUFFERS,
            kind: v4l2::BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            memory: v4l2::MEMORY_MMAP,
            ..Default::default()
        };
        ioctl(&video, v4l2::VIDIOC_REQBUFS, &mut request)?;
        let mut device = Self {
            camera,
            sensor,
            lens,
            video,
            buffers: Vec::new(),
            stride,
            held: None,
        };
        for index in 0..request.count {
            let mut planes = [Plane::default(); 1];
            let mut buffer = device.buffer(index, &mut planes);
            ioctl(&device.video, v4l2::VIDIOC_QUERYBUF, &mut buffer)?;
            let length = planes[0].length as usize;
            // SAFETY: maps the driver's buffer at the offset it gave.
            let data = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    device.video.as_raw_fd(),
                    planes[0].mem_offset as libc::off_t,
                )
            };
            if data == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            device.buffers.push(Mapping {
                data: data.cast(),
                length,
            });
            device.queue(index)?;
        }
        let mut kind = v4l2::BUF_TYPE_VIDEO_CAPTURE_MPLANE as i32;
        ioctl(&device.video, v4l2::VIDIOC_STREAMON, &mut kind)?;
        Ok(device)
    }

    fn buffer(&self, index: u32, planes: &mut [Plane; 1]) -> Buffer {
        Buffer {
            index,
            kind: v4l2::BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            bytesused: 0,
            flags: 0,
            field: 0,
            timestamp: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            timecode: [0; 4],
            sequence: 0,
            memory: v4l2::MEMORY_MMAP,
            planes: planes.as_mut_ptr(),
            length: 1,
            reserved2: 0,
            request_fd: 0,
        }
    }

    fn queue(&self, index: u32) -> io::Result<()> {
        let mut planes = [Plane::default(); 1];
        let mut buffer = self.buffer(index, &mut planes);
        ioctl(&self.video, v4l2::VIDIOC_QBUF, &mut buffer)
    }

    /// Waits up to a second for the next frame.
    pub fn frame(&mut self) -> io::Result<Frame<'_>> {
        if let Some(index) = self.held.take() {
            self.queue(index)?;
        }
        let mut poll = libc::pollfd {
            fd: self.video.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd for the video node.
        let ready = unsafe { libc::poll(&mut poll, 1, 1000) };
        if ready == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "no frame from the sensor",
            ));
        }
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut planes = [Plane::default(); 1];
        let mut buffer = self.buffer(0, &mut planes);
        ioctl(&self.video, v4l2::VIDIOC_DQBUF, &mut buffer)?;
        let index = buffer.index;
        self.held = Some(index);
        let mapping = &self.buffers[index as usize];
        let size = self.stride * self.camera.height as usize;
        // SAFETY: the driver filled this buffer and gave it back; it is not
        // queued again until the frame is dropped by the next `frame`.
        let data = unsafe { std::slice::from_raw_parts(mapping.data, size.min(mapping.length)) };
        Ok(Frame {
            data,
            stride: self.stride,
        })
    }

    /// Bytes per row of a frame.
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Sets the exposure in lines and the analogue gain, when its codes are
    /// known; the sensor takes them a few frames later.
    pub fn expose(&self, lines: u32, gain: f32) -> io::Result<()> {
        v4l2::set(&self.sensor, cid::EXPOSURE, lines as i32)?;
        match self.camera.gain_code(gain) {
            Some(code) => v4l2::set(&self.sensor, cid::ANALOGUE_GAIN, code as i32),
            None => Ok(()),
        }
    }

    /// Moves the lens; returns once the motor has taken the position.
    pub fn focus(&self, position: u32) -> io::Result<()> {
        match &self.lens {
            Some(lens) => v4l2::set(lens, cid::FOCUS_ABSOLUTE, position as i32),
            None => Ok(()),
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let mut kind = v4l2::BUF_TYPE_VIDEO_CAPTURE_MPLANE as i32;
        let _ = ioctl(&self.video, v4l2::VIDIOC_STREAMOFF, &mut kind);
        self.buffers.clear();
        let mut request = RequestBuffers {
            count: 0,
            kind: v4l2::BUF_TYPE_VIDEO_CAPTURE_MPLANE,
            memory: v4l2::MEMORY_MMAP,
            ..Default::default()
        };
        let _ = ioctl(&self.video, v4l2::VIDIOC_REQBUFS, &mut request);
    }
}
