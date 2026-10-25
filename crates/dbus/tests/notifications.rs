//! Настоящие вызовы и сигналы D-Bus на отдельной шине, включая перезапуск службы.
use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mimshel_dbus::notify::{self, CloseReason, NotifyEvent};
use zbus::blocking::{Proxy, connection::Builder};
use zbus::zvariant::{OwnedValue, StructureBuilder, Value};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn isolated_notification_roundtrip_and_restart() -> Result<()> {
    if std::env::var_os("MIMSHEL_TEST_PRIVATE_BUS").is_none() {
        let status = Command::new("timeout")
            .args(["30s", "dbus-run-session", "--"])
            .arg(std::env::current_exe()?)
            .args([
                "--exact",
                "isolated_notification_roundtrip_and_restart",
                "--nocapture",
            ])
            .env("MIMSHEL_TEST_PRIVATE_BUS", "1")
            .status()?;
        assert!(status.success(), "проверка отдельной D-Bus-шины: {status}");
        return Ok(());
    }
    let (tx, rx) = mpsc::channel();
    let server = notify::start(move |e| {
        let _ = tx.send(e);
    })?;
    let conn = Builder::session()?
        .method_timeout(Duration::from_secs(3))
        .build()?;
    let proxy = Proxy::new(&conn, NAME, PATH, NAME)?;
    let mut actions = proxy.receive_signal("ActionInvoked")?;
    let mut closed = proxy.receive_signal("NotificationClosed")?;

    let malformed: Value = StructureBuilder::new()
        .add_field(i32::MAX)
        .add_field(i32::MAX)
        .add_field(i32::MAX)
        .add_field(false)
        .add_field(8i32)
        .add_field(3i32)
        .add_field(Vec::<u8>::new())
        .build()?
        .into();
    let hints = HashMap::from([("image-data", OwnedValue::try_from(malformed)?)]);
    let id: u32 = proxy.call(
        "Notify",
        &(
            "test",
            0u32,
            "",
            "<b>Заголовок</b>",
            "Текст",
            vec!["default", "Открыть"],
            hints,
            5000i32,
        ),
    )?;
    assert_ne!(id, 0);
    let NotifyEvent::Show(n) = rx.recv_timeout(Duration::from_secs(3))? else {
        panic!("ожидалось уведомление")
    };
    assert_eq!(n.id, id);
    assert_eq!(n.summary, "Заголовок");
    assert!(n.image.is_none());

    let replacement: u32 = proxy.call(
        "Notify",
        &(
            "test",
            id,
            "",
            "Замена",
            "",
            Vec::<&str>::new(),
            HashMap::<&str, OwnedValue>::new(),
            -1i32,
        ),
    )?;
    assert_eq!(replacement, id);
    assert!(
        matches!(rx.recv_timeout(Duration::from_secs(3))?, NotifyEvent::Show(n) if n.summary == "Замена")
    );
    server.action(id, "default");
    let action: (u32, String) = actions
        .next()
        .ok_or("нет сигнала действия")?
        .body()
        .deserialize()?;
    assert_eq!(action, (id, "default".into()));
    let _: () = proxy.call("CloseNotification", &(id,))?;
    assert!(matches!(rx.recv_timeout(Duration::from_secs(3))?, NotifyEvent::Close(n) if n == id));
    server.closed(id, CloseReason::Closed);
    let closure: (u32, u32) = closed
        .next()
        .ok_or("нет сигнала закрытия")?
        .body()
        .deserialize()?;
    assert_eq!(closure, (id, 3));

    drop(server);
    let bus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while bus.name_has_owner(NAME.try_into()?)? {
        assert!(
            Instant::now() < deadline,
            "служба не освободила имя после остановки"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let (tx, rx) = mpsc::channel();
    let _restarted = notify::start(move |e| {
        let _ = tx.send(e);
    })?;
    // Используем того же клиента после смены владельца имени на шине.
    let _: u32 = proxy.call(
        "Notify",
        &(
            "test",
            0u32,
            "",
            "После перезапуска",
            "",
            Vec::<&str>::new(),
            HashMap::<&str, OwnedValue>::new(),
            -1i32,
        ),
    )?;
    assert!(
        matches!(rx.recv_timeout(Duration::from_secs(3))?, NotifyEvent::Show(n) if n.summary == "После перезапуска")
    );
    Ok(())
}
