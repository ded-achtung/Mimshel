//! A window's menu survives registrar restarts (the service restarting) on
//! a private bus: registered when the registrar appears after the window,
//! and again after the registrar goes away and comes back.
use std::process::Command;
use std::time::{Duration, Instant};

use mimshel_dbus::menu_export::{self, MenuSpec};
use mimshel_dbus::registrar::{self, Registrar, Windows};
use zbus::blocking::{Connection, connection::Builder};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const WINDOW: u32 = 0x4200007;

/// A registrar on its own connection, as the service runs it.
fn start_registrar() -> Result<(Connection, Windows)> {
    let windows = Windows::default();
    let conn = Builder::session()?
        .serve_at(
            registrar::PATH,
            Registrar {
                windows: windows.clone(),
            },
        )?
        .name(registrar::NAME)?
        .build()?;
    Ok((conn, windows))
}

/// Wait until the registrar knows the window's menu.
fn registered(windows: &Windows) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let known = windows
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&WINDOW)
            .is_some_and(|(bus, path)| !bus.is_empty() && path == "/MenuBar");
        if known {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn isolated_menu_reregisters_after_registrar_restart() -> Result<()> {
    if std::env::var_os("MIMSHEL_TEST_PRIVATE_BUS").is_none() {
        let status = Command::new("timeout")
            .args(["30s", "dbus-run-session", "--"])
            .arg(std::env::current_exe()?)
            .args([
                "--exact",
                "isolated_menu_reregisters_after_registrar_restart",
                "--nocapture",
            ])
            .env("MIMSHEL_TEST_PRIVATE_BUS", "1")
            .status()?;
        assert!(status.success(), "private D-Bus check: {status}");
        return Ok(());
    }
    // The window opens while the service is not running: no error, no menu yet.
    let menu = menu_export::export(
        WINDOW,
        vec![MenuSpec::menu(
            1,
            "Правка",
            vec![MenuSpec::item(10, "Копировать")],
        )],
        |_| {},
    )?;

    let (first, windows) = start_registrar()?;
    assert!(
        registered(&windows),
        "registered when the registrar appeared"
    );

    // The service restarts: the new registrar starts empty.
    drop(first);
    let (_second, windows) = start_registrar()?;
    assert!(registered(&windows), "registered again after the restart");

    drop(menu);
    Ok(())
}
