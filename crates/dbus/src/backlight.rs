//! Screen brightness through systemd-logind: `Session.SetBrightness` lets the
//! user of the active session change `/sys/class/backlight/*/brightness`,
//! which only root may write directly — no sudo, no udev rules.
//!
//! Dragging the slider sends many values: a worker thread keeps one system
//! bus connection and always writes only the latest value it has.

use std::sync::mpsc::{self, Receiver, Sender};

use zbus::blocking::Connection;

/// Writes brightness in the background (see the module docs).
pub struct Brightness(Sender<(String, u32)>);

impl Brightness {
    pub fn start() -> Brightness {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || worker(&rx));
        Brightness(tx)
    }

    /// Set backlight `device` to raw `value` (as in its `max_brightness`).
    pub fn set(&self, device: &str, value: u32) {
        let _ = self.0.send((device.to_string(), value));
    }
}

fn worker(rx: &Receiver<(String, u32)>) {
    let mut conn: Option<Connection> = None;
    while let Ok(mut next) = rx.recv() {
        // Skip to the newest request: the slider has moved on already.
        while let Ok(newer) = rx.try_recv() {
            next = newer;
        }
        if conn.is_none() {
            conn = Connection::system().ok();
        }
        let Some(c) = &conn else {
            eprintln!("Яркость: нет системной шины D-Bus");
            continue;
        };
        if let Err(e) = set_brightness(c, &next.0, next.1) {
            eprintln!("Яркость: logind не поменял {}: {e}", next.0);
            // The connection may be broken (logind restarted): reconnect next time.
            conn = None;
        }
    }
}

fn set_brightness(conn: &Connection, device: &str, value: u32) -> zbus::Result<()> {
    conn.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1/session/auto",
        Some("org.freedesktop.login1.Session"),
        "SetBrightness",
        &("backlight", device, value),
    )?;
    Ok(())
}
