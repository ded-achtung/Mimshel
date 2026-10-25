//! Bluetooth через BlueZ (`org.bluez` на системной шине): включён ли адаптер,
//! устройства (сопряжённые и найденные поиском), поиск, сопряжение,
//! подключение и отключение. Окна с кодом сопряжения показывает агент
//! blueman (`blueman-applet`) — он уже есть в сеансе.
//!
//! Всё — в отдельном потоке, как у трея: наружу — [`BtState`] через
//! `notify` (`None` — адаптера или службы bluetooth нет), внутрь — [`BtCmd`].

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Устройство Bluetooth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtDevice {
    /// Путь объекта BlueZ — по нему команды.
    pub path: String,
    pub name: String,
    /// Имя значка по стандарту (`audio-headset`, `input-mouse`…), может быть пустым.
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
}

/// Адаптер и устройства.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtState {
    pub powered: bool,
    pub discovering: bool,
    /// Сначала сопряжённые, потом найденные; внутри — по имени.
    pub devices: Vec<BtDevice>,
    /// Устройство, которое сейчас подключается или отключается.
    pub busy: Option<String>,
    /// Последняя ошибка словами (не вышло подключить).
    pub error: Option<String>,
}

/// Что сделать.
#[derive(Debug, Clone)]
pub enum BtCmd {
    Power(bool),
    /// Искать устройства (пока открыт список) или перестать.
    Scan(bool),
    /// Подключить; не сопряжено — сначала сопрячь и запомнить.
    Connect(String),
    Disconnect(String),
}

enum Wake {
    Changed,
    Cmd(BtCmd),
    /// Долгая операция закончилась: путь устройства и ошибка, если была.
    Done(String, Option<String>),
}

/// Работающая связь с Bluetooth: через неё — команды.
pub struct Bluetooth(Sender<Wake>);

impl Bluetooth {
    pub fn send(&self, cmd: BtCmd) {
        let _ = self.0.send(Wake::Cmd(cmd));
    }
}

/// Запустить. `notify` получает состояние (из потока Bluetooth) сразу и при
/// каждом изменении.
pub fn start(notify: impl Fn(Option<BtState>) + Send + 'static) -> zbus::Result<Bluetooth> {
    let conn = Connection::system()?;
    let (tx, rx) = mpsc::channel();
    {
        let conn = conn.clone();
        let tx = tx.clone();
        std::thread::spawn(move || listen(&conn, &tx));
    }
    let _ = tx.send(Wake::Changed);
    {
        let tx = tx.clone();
        std::thread::spawn(move || run(conn, rx, tx, notify));
    }
    Ok(Bluetooth(tx))
}

/// Сигналы BlueZ (свойства, новые и ушедшие устройства) — только будят.
fn listen(conn: &Connection, tx: &Sender<Wake>) {
    let rule = match MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BLUEZ)
    {
        Ok(b) => b.build(),
        Err(_) => return,
    };
    let Ok(iter) = MessageIterator::for_match_rule(rule, conn, Some(256)) else {
        return;
    };
    for _ in iter.flatten() {
        if tx.send(Wake::Changed).is_err() {
            return;
        }
    }
}

fn run(conn: Connection, rx: Receiver<Wake>, tx: Sender<Wake>, notify: impl Fn(Option<BtState>)) {
    let mut last: Option<Option<BtState>> = None;
    let mut busy: Option<String> = None;
    let mut error: Option<String> = None;
    while let Ok(first) = rx.recv() {
        // Пачку сигналов (при поиске их много) — одним перечитыванием.
        let mut cmds = Vec::new();
        let mut take = |w: Wake, busy: &mut Option<String>, error: &mut Option<String>| match w {
            Wake::Changed => {}
            Wake::Cmd(c) => cmds.push(c),
            Wake::Done(path, err) => {
                if busy.as_deref() == Some(path.as_str()) {
                    *busy = None;
                }
                *error = err;
            }
        };
        take(first, &mut busy, &mut error);
        while let Ok(w) = rx.recv_timeout(Duration::from_millis(50)) {
            take(w, &mut busy, &mut error);
        }
        for cmd in cmds {
            command(&conn, cmd, &mut busy, &mut error, &tx);
        }
        let now = read(&conn).map(|mut s| {
            s.busy = busy.clone();
            s.error = error.clone();
            s
        });
        if last.as_ref() != Some(&now) {
            last = Some(now.clone());
            notify(now);
        }
    }
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// Адаптер и устройства одним запросом (ObjectManager).
fn read(conn: &Connection) -> Option<BtState> {
    let reply = conn
        .call_method(
            Some(BLUEZ),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .ok()?;
    let objects: Objects = reply.body().deserialize().ok()?;
    let mut adapter = None;
    let mut devices = Vec::new();
    for (path, ifaces) in &objects {
        if let Some(a) = ifaces.get(ADAPTER) {
            adapter = Some((flag(a, "Powered"), flag(a, "Discovering")));
        }
        if let Some(d) = ifaces.get(DEVICE) {
            let paired = flag(d, "Paired");
            // Найденное без имени (у многих телефонов и браслетов только
            // адрес) не показываем — выбрать его всё равно не по чему.
            let name = text(d, "Name").filter(|n| !n.is_empty());
            let name = match name {
                Some(n) => n,
                None if paired => text(d, "Alias").unwrap_or_default(),
                None => continue,
            };
            devices.push(BtDevice {
                path: path.to_string(),
                name,
                icon: text(d, "Icon").unwrap_or_default(),
                paired,
                connected: flag(d, "Connected"),
            });
        }
    }
    let (powered, discovering) = adapter?;
    sort_devices(&mut devices);
    Some(BtState {
        powered,
        discovering,
        devices,
        busy: None,
        error: None,
    })
}

/// Сопряжённые — первыми, дальше по имени без учёта регистра.
fn sort_devices(devices: &mut [BtDevice]) {
    devices.sort_by(|a, b| {
        b.paired
            .cmp(&a.paired)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    props
        .get(key)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

fn text(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| String::try_from(v.clone()).ok())
}

/// Путь адаптера: устройства лежат под ним (`/org/bluez/hci0/dev_…`).
fn adapter_path(conn: &Connection) -> Option<String> {
    let reply = conn
        .call_method(
            Some(BLUEZ),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .ok()?;
    let objects: Objects = reply.body().deserialize().ok()?;
    objects
        .iter()
        .find(|(_, i)| i.contains_key(ADAPTER))
        .map(|(p, _)| p.to_string())
}

fn command(
    conn: &Connection,
    cmd: BtCmd,
    busy: &mut Option<String>,
    error: &mut Option<String>,
    tx: &Sender<Wake>,
) {
    match cmd {
        BtCmd::Power(on) => {
            if let Some(adapter) = adapter_path(conn) {
                let _ = conn.call_method(
                    Some(BLUEZ),
                    adapter.as_str(),
                    Some(PROPERTIES),
                    "Set",
                    &(ADAPTER, "Powered", Value::from(on)),
                );
            }
        }
        BtCmd::Scan(on) => {
            if let Some(adapter) = adapter_path(conn) {
                let method = if on {
                    "StartDiscovery"
                } else {
                    "StopDiscovery"
                };
                // Уже ищет (или уже не ищет) — BlueZ отвечает ошибкой, это не беда.
                let _ = conn.call_method(Some(BLUEZ), adapter.as_str(), Some(ADAPTER), method, &());
            }
        }
        BtCmd::Connect(path) | BtCmd::Disconnect(path) if busy.is_some() => {
            // Одно устройство за раз: BlueZ всё равно делает по очереди.
            let _ = path;
        }
        BtCmd::Connect(path) => {
            *busy = Some(path.clone());
            *error = None;
            let conn = conn.clone();
            let tx = tx.clone();
            // Сопряжение и подключение идут секунды (ждут ответа устройства
            // и человека — код в окне blueman): не держим поток.
            std::thread::spawn(move || {
                let err = connect(&conn, &path).err();
                let _ = tx.send(Wake::Done(path, err));
            });
        }
        BtCmd::Disconnect(path) => {
            *busy = Some(path.clone());
            *error = None;
            let conn = conn.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let err = conn
                    .call_method(Some(BLUEZ), path.as_str(), Some(DEVICE), "Disconnect", &())
                    .err()
                    .map(|e| format!("Не удалось отключить: {}", short(&e)));
                let _ = tx.send(Wake::Done(path, err));
            });
        }
    }
}

/// Сопрячь (если ещё нет), запомнить как доверенное и подключить.
fn connect(conn: &Connection, path: &str) -> Result<(), String> {
    let call = |iface: &str, method: &str| {
        conn.call_method(Some(BLUEZ), path, Some(iface), method, &())
            .map(|_| ())
    };
    let paired = conn
        .call_method(
            Some(BLUEZ),
            path,
            Some(PROPERTIES),
            "Get",
            &(DEVICE, "Paired"),
        )
        .ok()
        .and_then(|r| r.body().deserialize::<OwnedValue>().ok())
        .and_then(|v| bool::try_from(&v).ok())
        .unwrap_or(false);
    if !paired {
        call(DEVICE, "Pair").map_err(|e| format!("Не удалось сопрячь: {}", short(&e)))?;
    }
    // Доверенное устройство подключается само, когда включится рядом.
    let _ = conn.call_method(
        Some(BLUEZ),
        path,
        Some(PROPERTIES),
        "Set",
        &(DEVICE, "Trusted", Value::from(true)),
    );
    call(DEVICE, "Connect").map_err(|e| format!("Не удалось подключить: {}", short(&e)))
}

/// Ошибка BlueZ коротко: без имени ошибки D-Bus.
fn short(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(_, Some(text), _) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, paired: bool) -> BtDevice {
        BtDevice {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.into(),
            icon: String::new(),
            paired,
            connected: false,
        }
    }

    #[test]
    fn paired_first_then_by_name() {
        let mut list = vec![
            dev("колонка", false),
            dev("Наушники", true),
            dev("Мышь", true),
            dev("Аудио", false),
        ];
        sort_devices(&mut list);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Мышь", "Наушники", "Аудио", "колонка"]);
    }
}
