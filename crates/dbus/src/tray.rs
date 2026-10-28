//! Трей по стандарту StatusNotifierItem (KDE/freedesktop): программы
//! регистрируют значки у «наблюдателя» (`org.kde.StatusNotifierWatcher`),
//! а «хозяин» (мы же) читает их свойства — иконку, подсказку, меню — и
//! передаёт щелчки обратно. Старое встраивание окон (XEmbed) не делаем —
//! решение пользователя: nm-applet запускается с `--indicator`, blueman
//! сам переходит на этот способ, когда наблюдатель есть.
//!
//! Всё — в отдельном потоке; наружу — [`TrayUpdate`] через `notify`,
//! внутрь — [`TrayCmd`] через [`Tray::send`].

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::{Header, Type as MessageType};
use zbus::zvariant::{OwnedValue, Value};
use zbus::{MatchRule, interface};

use crate::menu::{Layout, MenuItem, parse_layout};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const DBUSMENU: &str = "com.canonical.dbusmenu";

/// Значок трея, как его видно в строке меню.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayItem {
    /// Чей значок: «имя на шине + путь» — по нему же команды.
    pub key: String,
    pub title: String,
    /// `Passive` — значок прятать; `NeedsAttention` — показывать иконку внимания.
    pub status: String,
    pub icon_name: String,
    /// Дополнительная папка с иконками от программы.
    pub icon_theme_path: String,
    /// Готовая картинка (если имени иконки нет): ширина, высота, ARGB
    /// построчно, байты в порядке «от старшего» (как в стандарте).
    pub pixmap: Option<(u32, u32, Vec<u8>)>,
    pub tooltip: String,
    /// Значок — это только меню (щелчок открывает меню, а не программу).
    pub item_is_menu: bool,
    pub has_menu: bool,
}

/// Что сообщает трей.
#[derive(Debug, Clone)]
pub enum TrayUpdate {
    Items(Vec<TrayItem>),
    /// Пункты меню значка (ответ на [`TrayCmd::Menu`]).
    Menu {
        key: String,
        items: Vec<MenuItem>,
    },
}

/// Что попросить у значка. Координаты — точка экрана, где щёлкнули.
#[derive(Debug, Clone)]
pub enum TrayCmd {
    Activate(String, i32, i32),
    SecondaryActivate(String, i32, i32),
    /// Прокрутка колёсиком: шаг и направление («vertical»).
    Scroll(String, i32),
    /// Прочитать меню значка.
    Menu(String),
    /// Пункт меню выбран.
    MenuClicked(String, i32),
}

/// Что будит поток трея: значки поменялись или пришла команда.
enum Wake {
    Changed,
    Cmd(TrayCmd),
}

/// Трей запущен: через него — команды значкам.
pub struct Tray(Sender<Wake>);

impl Tray {
    pub fn send(&self, cmd: TrayCmd) {
        let _ = self.0.send(Wake::Cmd(cmd));
    }
}

/// Наблюдатель: принимает регистрации значков.
struct Watcher {
    items: Arc<Mutex<Vec<(String, String)>>>,
    changed: Sender<Wake>,
}

/// Tray items kept at most.
const MAX_ITEMS: usize = 64;

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// Программа регистрирует значок: имя на шине (тогда путь —
    /// `/StatusNotifierItem`) или путь (тогда имя — отправителя).
    fn register_status_notifier_item(&self, service: &str, #[zbus(header)] header: Header<'_>) {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let (bus, path) = if service.starts_with('/') {
            (sender, service.to_string())
        } else {
            (service.to_string(), "/StatusNotifierItem".to_string())
        };
        let mut items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        // An item is dropped when its bus name goes away; a name that was
        // never owned never goes away, so the list is capped.
        if items.len() >= MAX_ITEMS {
            return;
        }
        if !items.contains(&(bus.clone(), path.clone())) {
            items.push((bus, path));
            let _ = self.changed.send(Wake::Changed);
        }
    }

    fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        let items = self.items.lock().unwrap_or_else(|p| p.into_inner());
        items.iter().map(|(b, p)| format!("{b}{p}")).collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

/// Запустить трей. `notify` получает обновления (из потока трея).
/// Имя наблюдателя занято — ждём в очереди, пока его не освободят.
pub fn start(notify: impl Fn(TrayUpdate) + Send + 'static) -> zbus::Result<Tray> {
    let items = Arc::new(Mutex::new(Vec::new()));
    let (wake_tx, wake_rx) = mpsc::channel();
    let conn = crate::session()?;
    conn.object_server().at(
        "/StatusNotifierWatcher",
        Watcher {
            items: items.clone(),
            changed: wake_tx.clone(),
        },
    )?;
    // Имя занято (трей панели Xfce, которую вот-вот остановят) — встаём в
    // очередь: имя перейдёт к нам, а программы сами перерегистрируют значки.
    let reply =
        conn.request_name_with_flags(WATCHER, zbus::fdo::RequestNameFlags::ReplaceExisting.into())?;
    eprintln!("Трей: {reply:?}");

    // Сигналы значков и уход программ с шины — отдельный поток: он только
    // будит основной.
    {
        let conn = conn.clone();
        let items = items.clone();
        let wake = wake_tx.clone();
        std::thread::spawn(move || listen(&conn, &items, &wake));
    }
    std::thread::spawn(move || run(conn, items, wake_rx, notify));
    Ok(Tray(wake_tx))
}

/// Сигналы: значок поменялся — перечитать; программа ушла — убрать её значки.
fn listen(conn: &Connection, items: &Mutex<Vec<(String, String)>>, changed: &Sender<Wake>) {
    let rule = MatchRule::builder().msg_type(MessageType::Signal).build();
    let Ok(iter) = MessageIterator::for_match_rule(rule, conn, Some(256)) else {
        return;
    };
    for msg in iter.flatten() {
        let header = msg.header();
        let iface = header
            .interface()
            .map(|i| i.as_str().to_string())
            .unwrap_or_default();
        let member = header
            .member()
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        if iface == ITEM {
            let _ = changed.send(Wake::Changed);
        } else if iface == "org.freedesktop.DBus" && member == "NameOwnerChanged" {
            let Ok((name, _old, new)) = msg.body().deserialize::<(String, String, String)>() else {
                continue;
            };
            if new.is_empty() {
                let mut list = items.lock().unwrap_or_else(|p| p.into_inner());
                let before = list.len();
                list.retain(|(bus, _)| *bus != name);
                if list.len() != before && changed.send(Wake::Changed).is_err() {
                    return;
                }
            }
        }
    }
}

fn run(
    conn: Connection,
    items: Arc<Mutex<Vec<(String, String)>>>,
    wake: Receiver<Wake>,
    notify: impl Fn(TrayUpdate),
) {
    let mut last: Vec<TrayItem> = Vec::new();
    while let Ok(first) = wake.recv() {
        // Пачку изменений — одним перечитыванием (значки шлют по нескольку
        // сигналов подряд); команды — по порядку.
        let mut dirty = false;
        let mut cmds = Vec::new();
        let mut take = |w: Wake| match w {
            Wake::Changed => dirty = true,
            Wake::Cmd(c) => cmds.push(c),
        };
        take(first);
        while let Ok(w) = wake.recv_timeout(Duration::from_millis(30)) {
            take(w);
        }
        if dirty {
            let list = items.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let now: Vec<TrayItem> = list
                .iter()
                .filter_map(|(bus, path)| read_item(&conn, bus, path))
                .collect();
            if now != last {
                last = now.clone();
                notify(TrayUpdate::Items(now));
            }
        }
        for cmd in cmds {
            command(&conn, cmd, &notify);
        }
    }
}

fn split_key(key: &str) -> Option<(&str, &str)> {
    let i = key.find('/')?;
    Some((&key[..i], &key[i..]))
}

fn command(conn: &Connection, cmd: TrayCmd, notify: &impl Fn(TrayUpdate)) {
    let call = |key: &str, method: &str, body: &dyn Fn(&str, &str) -> zbus::Result<()>| {
        if let Some((bus, path)) = split_key(key)
            && let Err(e) = body(bus, path)
        {
            eprintln!("Трей: {method} у «{key}» не вышло: {e}");
        }
    };
    match cmd {
        TrayCmd::Activate(key, x, y) => {
            let done = split_key(&key).is_some_and(|(bus, path)| {
                conn.call_method(Some(bus), path, Some(ITEM), "Activate", &(x, y))
                    .is_ok()
            });
            // Действия на щелчок нет (так у значков appindicator, например
            // nm-applet) — показать меню, как это делают другие панели.
            if !done {
                command(conn, TrayCmd::Menu(key), notify);
            }
        }
        TrayCmd::SecondaryActivate(key, x, y) => call(&key, "SecondaryActivate", &|bus, path| {
            conn.call_method(Some(bus), path, Some(ITEM), "SecondaryActivate", &(x, y))
                .map(|_| ())
        }),
        TrayCmd::Scroll(key, delta) => call(&key, "Scroll", &|bus, path| {
            conn.call_method(Some(bus), path, Some(ITEM), "Scroll", &(delta, "vertical"))
                .map(|_| ())
        }),
        TrayCmd::Menu(key) => {
            let menu = menu_path(conn, &key).and_then(|(bus, path)| {
                // Программа может достроить меню перед показом.
                let _ = conn.call_method(
                    Some(bus.as_str()),
                    path.as_str(),
                    Some(DBUSMENU),
                    "AboutToShow",
                    &(0i32,),
                );
                let reply = conn
                    .call_method(
                        Some(bus.as_str()),
                        path.as_str(),
                        Some(DBUSMENU),
                        "GetLayout",
                        &(0i32, -1i32, Vec::<String>::new()),
                    )
                    .ok()?;
                let (_rev, layout): (u32, Layout) = reply.body().deserialize().ok()?;
                Some(parse_layout(layout))
            });
            notify(TrayUpdate::Menu {
                key,
                items: menu.unwrap_or_default(),
            });
        }
        TrayCmd::MenuClicked(key, id) => {
            if let Some((bus, path)) = menu_path(conn, &key) {
                let now = 0u32;
                let data = Value::from("");
                if let Err(e) = conn.call_method(
                    Some(bus.as_str()),
                    path.as_str(),
                    Some(DBUSMENU),
                    "Event",
                    &(id, "clicked", data, now),
                ) {
                    eprintln!("Трей: пункт меню у «{key}» не сработал: {e}");
                }
            }
        }
    }
}

/// Где меню значка: имя на шине и путь объекта меню.
fn menu_path(conn: &Connection, key: &str) -> Option<(String, String)> {
    let (bus, path) = split_key(key)?;
    let props = get_all(conn, bus, path)?;
    let menu = props.get("Menu")?;
    let menu = zbus::zvariant::OwnedObjectPath::try_from(menu.try_clone().ok()?).ok()?;
    Some((bus.to_string(), menu.as_str().to_string()))
}

fn get_all(conn: &Connection, bus: &str, path: &str) -> Option<HashMap<String, OwnedValue>> {
    conn.call_method(
        Some(bus),
        path,
        Some("org.freedesktop.DBus.Properties"),
        "GetAll",
        &(ITEM,),
    )
    .ok()?
    .body()
    .deserialize()
    .ok()
}

fn read_item(conn: &Connection, bus: &str, path: &str) -> Option<TrayItem> {
    let props = get_all(conn, bus, path)?;
    let text = |name: &str| {
        props
            .get(name)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default()
    };
    let status = text("Status");
    let icon_name = if status == "NeedsAttention" && !text("AttentionIconName").is_empty() {
        text("AttentionIconName")
    } else {
        text("IconName")
    };
    let pixmap = props
        .get("IconPixmap")
        .and_then(|v| Vec::<(i32, i32, Vec<u8>)>::try_from(v.try_clone().ok()?).ok())
        .and_then(|list| {
            // Самая большая картинка — уменьшить лучше, чем растянуть.
            list.into_iter()
                // i64: a hostile size like 65536×65536 overflowed the i32
                // product (a panic in debug builds, a wrong match in release).
                .filter(|(w, h, d)| {
                    *w > 0 && *h > 0 && d.len() as i64 == i64::from(*w) * i64::from(*h) * 4
                })
                .max_by_key(|(w, _, _)| *w)
                .map(|(w, h, d)| (w as u32, h as u32, d))
        });
    let tooltip = props
        .get("ToolTip")
        .and_then(|v| {
            <(String, Vec<(i32, i32, Vec<u8>)>, String, String)>::try_from(v.try_clone().ok()?).ok()
        })
        .map(|(_, _, title, body)| {
            if body.is_empty() {
                title
            } else {
                format!("{title}\n{body}")
            }
        })
        .unwrap_or_default();
    let flag = |name: &str| {
        props
            .get(name)
            .and_then(|v| bool::try_from(v.try_clone().ok()?).ok())
            .unwrap_or(false)
    };
    Some(TrayItem {
        key: format!("{bus}{path}"),
        title: text("Title"),
        status,
        icon_name,
        icon_theme_path: text("IconThemePath"),
        pixmap,
        tooltip,
        item_is_menu: flag("ItemIsMenu"),
        has_menu: props.contains_key("Menu"),
    })
}
