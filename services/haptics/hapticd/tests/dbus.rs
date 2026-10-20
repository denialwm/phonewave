//! Real private D-Bus transport and file-backed hardware contract; no motor.
use hapticd::{BUS_NAME, DeviceInfo, INTERFACE, OBJECT_PATH};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use zbus::blocking::{Connection, Proxy, connection::Builder};

struct Fixture {
    daemon: Child,
    bus: Child,
    root: PathBuf,
    address: String,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("hapticd-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for file in ["duration", "activate", "gain"] {
            fs::write(root.join(file), "0\n").unwrap();
        }
        let config = root.join("config.json");
        fs::write(&config, serde_json::json!({"devices":[{"id":"test", "backend":"sysfs", "path":root, "gain":"gain", "gain_max":128}]}).to_string()).unwrap();
        let mut bus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon is required for integration tests");
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_string();
        let daemon = Command::new(env!("CARGO_BIN_EXE_hapticd"))
            .args(["--session", "--config"])
            .arg(&config)
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .spawn()
            .unwrap();
        let fixture = Self {
            daemon,
            bus,
            root,
            address,
        };
        let connection = fixture.connect();
        let bus = zbus::blocking::fdo::DBusProxy::new(&connection).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bus.name_has_owner(BUS_NAME.try_into().unwrap()).unwrap() {
            assert!(Instant::now() < deadline, "service did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        fixture
    }
    fn connect(&self) -> Connection {
        Builder::address(self.address.as_str())
            .unwrap()
            .method_timeout(Duration::from_secs(2))
            .build()
            .unwrap()
    }
    fn wait_value(&self, name: &str, value: &str) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while fs::read_to_string(self.root.join(name)).unwrap() != value {
            assert!(Instant::now() < deadline, "{name} did not become {value}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: signal only the child process owned by this fixture.
        unsafe {
            libc::kill(self.daemon.id() as i32, libc::SIGTERM);
        }
        let _ = self.daemon.wait();
        let _ = self.bus.kill();
        let _ = self.bus.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn proxy(connection: &Connection) -> Proxy<'_> {
    Proxy::new(connection, BUS_NAME, OBJECT_PATH, INTERFACE).unwrap()
}

#[test]
fn dbus_controls_gain_duration_patterns_and_client_lifetime() {
    let fixture = Fixture::new();
    let first = fixture.connect();
    let second = fixture.connect();
    let p1 = proxy(&first);
    let p2 = proxy(&second);
    let devices: Vec<DeviceInfo> = p1.call("ListDevices", &()).unwrap();
    assert_eq!(devices.len(), 1);
    assert!(devices[0].intensity);
    let id: u64 = p1.call("Vibrate", &("", 1000u32, 0.5f64)).unwrap();
    fixture.wait_value("gain", "64\n");
    fixture.wait_value("duration", "1000\n");
    fixture.wait_value("activate", "1\n");
    assert!(p2.call::<_, _, ()>("Cancel", &(id,)).is_err());
    assert!(
        p1.call::<_, _, u64>("Vibrate", &("", 0u32, 1.0f64))
            .is_err()
    );
    assert!(
        p1.call::<_, _, u64>("Vibrate", &("", 1u32, f64::NAN))
            .is_err()
    );
    assert!(
        p1.call::<_, _, u64>("Vibrate", &("missing", 10u32, 1.0f64))
            .is_err()
    );
    p1.call::<_, _, ()>("Cancel", &(id,)).unwrap();
    fixture.wait_value("activate", "0\n");

    let _: u64 = p1
        .call(
            "PlayPattern",
            &("test", vec![(20u32, 0.25f64), (50, 0.0), (20, 1.0)]),
        )
        .unwrap();
    fixture.wait_value("gain", "32\n");
    fixture.wait_value("gain", "128\n");
    fixture.wait_value("activate", "0\n");
    p1.call::<_, _, ()>("Rescan", &()).unwrap();

    let _: u64 = p1.call("Vibrate", &("", 1000u32, 0.75f64)).unwrap();
    fixture.wait_value("activate", "1\n");
    drop(p1);
    first.close().unwrap();
    fixture.wait_value("activate", "0\n");

    // A missing attribute causes a D-Bus error and does not kill the daemon.
    fs::remove_file(fixture.root.join("gain")).unwrap();
    fs::create_dir(fixture.root.join("gain")).unwrap();
    assert!(
        p2.call::<_, _, u64>("Vibrate", &("", 10u32, 1.0f64))
            .is_err()
    );
    let _: Vec<DeviceInfo> = p2.call("ListDevices", &()).unwrap();
    fixture.wait_value("activate", "0\n");
}
