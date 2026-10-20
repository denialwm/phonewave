//! Optional real-kernel force-feedback test with a virtual, motorless actuator.
use hapticd::{Config, DeviceConfig, backend};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

nix::ioctl_write_int!(set_evbit, b'U', 100);
nix::ioctl_write_int!(set_ffbit, b'U', 107);
nix::ioctl_write_ptr!(setup, b'U', 3, libc::uinput_setup);
nix::ioctl_none!(create, b'U', 1);
nix::ioctl_none!(destroy, b'U', 2);
nix::ioctl_read_buf!(sysname, b'U', 44, u8);
nix::ioctl_readwrite!(begin_upload, b'U', 200, libc::uinput_ff_upload);
nix::ioctl_write_ptr!(end_upload, b'U', 201, libc::uinput_ff_upload);
nix::ioctl_readwrite!(begin_erase, b'U', 202, libc::uinput_ff_erase);
nix::ioctl_write_ptr!(end_erase, b'U', 203, libc::uinput_ff_erase);

struct VirtualDevice {
    file: File,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for VirtualDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // SAFETY: this fd owns only the virtual actuator created by this test.
        let _ = unsafe { destroy(self.file.as_raw_fd()) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
#[ignore = "requires /dev/uinput and permission to open its generated event node"]
fn kernel_evdev_upload_play_stop_and_cleanup() {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
        .unwrap();
    let mut driver = VirtualDevice {
        file,
        stop: Arc::new(AtomicBool::new(false)),
        worker: None,
    };
    let fd = driver.file.as_raw_fd();
    // SAFETY: zero is valid for these integer fields and fixed-size name array.
    let mut description: libc::uinput_setup = unsafe { std::mem::zeroed() };
    description.id.bustype = 0x06;
    description.ff_effects_max = 4;
    for (out, byte) in description.name.iter_mut().zip(b"hapticd-test-actuator") {
        *out = *byte as libc::c_char;
    }
    let mut name = [0u8; 128];
    // SAFETY: all ioctls operate on the test-owned uinput descriptor with
    // correctly sized structures/buffers; no input keys or axes are exposed.
    unsafe {
        set_evbit(fd, 0x15).unwrap();
        set_ffbit(fd, 0x50).unwrap();
        setup(fd, &description).unwrap();
        create(fd).unwrap();
        sysname(fd, &mut name).unwrap();
    }
    let name = std::str::from_utf8(name.split(|b| *b == 0).next().unwrap()).unwrap();
    let directory = PathBuf::from("/sys/class/input").join(name);
    let deadline = Instant::now() + Duration::from_secs(3);
    let path = loop {
        if let Some(event) = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().as_encoded_bytes().starts_with(b"event"))
        {
            let path = PathBuf::from("/dev/input").join(event.file_name());
            if OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .is_ok()
            {
                break path;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no accessible virtual evdev node"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let stop = driver.stop.clone();
    let mut stream = driver.file.try_clone().unwrap();
    let (tx, rx) = mpsc::channel();
    driver.worker = Some(thread::spawn(move || {
        let fd = stream.as_raw_fd();
        while !stop.load(Ordering::Relaxed) {
            let mut poll = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one initialized pollfd is supplied for the duration of poll.
            if unsafe { libc::poll(&mut poll, 1, 20) } <= 0 {
                continue;
            }
            // SAFETY: zeroed input_event is valid and the byte slice covers it.
            let mut event: libc::input_event = unsafe { std::mem::zeroed() };
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(
                    std::ptr::from_mut(&mut event).cast::<u8>(),
                    std::mem::size_of_val(&event),
                )
            };
            if stream.read_exact(bytes).is_err() {
                break;
            }
            if event.type_ == 0x101 && event.code == 1 {
                // SAFETY: valid zeroed upload; the kernel fills the effect for
                // this request ID, and the completion returns retval=0.
                unsafe {
                    let mut upload: libc::uinput_ff_upload = std::mem::zeroed();
                    upload.request_id = event.value as u32;
                    begin_upload(fd, &mut upload).unwrap();
                    let rumble = &*upload.effect.u.as_ptr().cast::<libc::ff_rumble_effect>();
                    tx.send((
                        upload.effect.replay.length as u32,
                        rumble.strong_magnitude,
                        rumble.weak_magnitude,
                    ))
                    .unwrap();
                    upload.retval = 0;
                    end_upload(fd, &upload).unwrap();
                }
            } else if event.type_ == 0x101 && event.code == 2 {
                // SAFETY: same request/complete protocol for an effect erase.
                unsafe {
                    let mut erase: libc::uinput_ff_erase = std::mem::zeroed();
                    erase.request_id = event.value as u32;
                    begin_erase(fd, &mut erase).unwrap();
                    erase.retval = 0;
                    end_erase(fd, &erase).unwrap();
                }
            } else if event.type_ == 0x15 {
                tx.send((0, event.value as u16, 0)).unwrap();
            }
        }
    }));
    let config = Config {
        devices: vec![DeviceConfig {
            id: "virtual".into(),
            backend: "evdev".into(),
            path,
            gain: None,
            gain_max: None,
        }],
        ..Config::default()
    };
    let mut devices = backend::discover(&config).unwrap();
    let device = devices.get_mut("virtual").unwrap();
    device.backend.play(37, 0.5).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        (37, 32768, 32768)
    );
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), (0, 1, 0));
    device.backend.stop().unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), (0, 0, 0));
    drop(devices);
}
