//! A program that never answers must not hang our services: calls made
//! over `mimshel_dbus::session()` give up after `CALL_TIMEOUT`.
use std::process::Command;
use std::time::{Duration, Instant};

use zbus::blocking::connection::Builder;
use zbus::interface;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const NAME: &str = "org.mimshel.TestHung";
const PATH: &str = "/org/mimshel/TestHung";

/// A service whose only method takes far longer than the timeout.
struct Hung;

#[interface(name = "org.mimshel.TestHung")]
impl Hung {
    fn wait(&self) -> u32 {
        std::thread::sleep(Duration::from_secs(20));
        0
    }
}

#[test]
fn call_to_a_hung_program_times_out() -> Result<()> {
    if std::env::var_os("MIMSHEL_TEST_PRIVATE_BUS").is_none() {
        let status = Command::new("timeout")
            .args(["30s", "dbus-run-session", "--"])
            .arg(std::env::current_exe()?)
            .args(["--exact", "call_to_a_hung_program_times_out", "--nocapture"])
            .env("MIMSHEL_TEST_PRIVATE_BUS", "1")
            .status()?;
        assert!(status.success(), "private D-Bus check: {status}");
        return Ok(());
    }
    let _service = Builder::session()?
        .serve_at(PATH, Hung)?
        .name(NAME)?
        .build()?;
    let conn = mimshel_dbus::session()?;
    let start = Instant::now();
    let reply = conn.call_method(Some(NAME), PATH, Some(NAME), "Wait", &());
    let took = start.elapsed();
    assert!(reply.is_err(), "a hung call must fail, got {reply:?}");
    assert!(
        took < mimshel_dbus::CALL_TIMEOUT + Duration::from_secs(2),
        "gave up after {took:?}"
    );
    Ok(())
}
