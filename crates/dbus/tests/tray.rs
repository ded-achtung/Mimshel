//! The tray on a private bus: an item registers, changes, a made-up
//! registration is forgotten, and a program that leaves takes its icon along.
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mimshel_dbus::tray::{self, TrayUpdate};
use zbus::blocking::{Proxy, connection::Builder};
use zbus::interface;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Item {
    icon: Arc<Mutex<String>>,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn id(&self) -> String {
        "test-item".into()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "Active".into()
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.icon.lock().unwrap().clone()
    }
}

/// The next list of items, skipping menu answers.
fn items(rx: &mpsc::Receiver<TrayUpdate>) -> Result<Vec<tray::TrayItem>> {
    loop {
        if let TrayUpdate::Items(list) = rx.recv_timeout(Duration::from_secs(5))? {
            return Ok(list);
        }
    }
}

#[test]
fn isolated_tray_items_come_change_and_go() -> Result<()> {
    if std::env::var_os("MIMSHEL_TEST_PRIVATE_BUS").is_none() {
        let status = Command::new("timeout")
            .args(["30s", "dbus-run-session", "--"])
            .arg(std::env::current_exe()?)
            .args([
                "--exact",
                "isolated_tray_items_come_change_and_go",
                "--nocapture",
            ])
            .env("MIMSHEL_TEST_PRIVATE_BUS", "1")
            .status()?;
        assert!(status.success(), "the tray on a private bus: {status}");
        return Ok(());
    }
    let (tx, rx) = mpsc::channel();
    let _tray = tray::start(move |update| {
        let _ = tx.send(update);
    })?;

    // A program with a tray icon.
    let icon = Arc::new(Mutex::new(String::from("first-icon")));
    let app = Builder::session()?
        .serve_at("/StatusNotifierItem", Item { icon: icon.clone() })?
        .build()?;
    let name = app.unique_name().ok_or("no bus name")?.to_string();
    let watcher = Proxy::new(
        &app,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )?;
    watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),))?;
    let list = items(&rx)?;
    assert_eq!(list.len(), 1);
    assert_eq!(
        (list[0].id.as_str(), list[0].icon_name.as_str()),
        ("test-item", "first-icon")
    );

    // A name nobody owns: registered, then forgotten at the next reading.
    watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &("made.up.name",))?;
    // The icon changes: the signal reaches the tray through its narrow
    // subscription and the item is read again.
    *icon.lock().unwrap() = "second-icon".into();
    app.emit_signal(
        None::<&str>,
        "/StatusNotifierItem",
        "org.kde.StatusNotifierItem",
        "NewIcon",
        &(),
    )?;
    let list = items(&rx)?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].icon_name, "second-icon");
    let registered: Vec<String> = watcher.get_property("RegisteredStatusNotifierItems")?;
    assert_eq!(
        registered.len(),
        1,
        "the made-up name is forgotten: {registered:?}"
    );

    // The program leaves the bus: its icon goes.
    drop(watcher);
    drop(app);
    assert!(items(&rx)?.is_empty());
    Ok(())
}
