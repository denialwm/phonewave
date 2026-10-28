//! braun: the camera service.

use std::process::ExitCode;

use braun::{bus, session};

fn main() -> ExitCode {
    let _connection = match bus::serve() {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("braun: D-Bus: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("braun: on the system bus");
    // Find the cameras now, so the first client does not wait for it.
    session::cameras(false);
    loop {
        std::thread::park();
    }
}
