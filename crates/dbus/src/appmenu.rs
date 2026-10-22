//! Глобальное меню: пункты меню активного окна («Файл», «Правка»…) — чтобы
//! показать их в строке меню, как на Mac. Программы отдают меню двумя
//! способами:
//! - **GTK** (`org.gtk.Menus` + `org.gtk.Actions`): где искать — в свойствах
//!   окна X11 (`_GTK_UNIQUE_BUS_NAME`, `_GTK_MENUBAR_OBJECT_PATH`, пути
//!   действий программы, окна и модуля appmenu-gtk-module). Так — Mousepad,
//!   Thunar и прочие GTK-программы (модуль включён в `GTK_MODULES`).
//! - **DBusMenu через регистратор** (`com.canonical.AppMenu.Registrar`):
//!   Qt и другие. Регистратор — наш же ([`crate::registrar`]); пока он есть,
//!   GTK-программы прячут строки меню в окнах.
//!
//! Работает в своём потоке; наружу — [`AppMenuUpdate`], внутрь — [`AppMenuCmd`].

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Signature, Value};

use crate::menu::{Layout, MenuItem, parse_layout, strip_mnemonic};
use crate::registrar::{self, Registrar, Windows};

const GTK_MENUS: &str = "org.gtk.Menus";
const GTK_ACTIONS: &str = "org.gtk.Actions";
const DBUSMENU: &str = "com.canonical.dbusmenu";
const REGISTRAR: &str = "com.canonical.AppMenu.Registrar";

/// Где меню окна по способу GTK (из свойств окна X11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GtkPaths {
    pub bus: String,
    pub menubar: String,
    /// Пути действий: `app.…`, `win.…`, `unity.…` (appmenu-gtk-module).
    pub app: Option<String>,
    pub window: Option<String>,
    pub unity: Option<String>,
}

/// Откуда брать меню окна.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Gtk(GtkPaths),
    /// Спросить регистратор по номеру окна X11.
    Registrar,
}

#[derive(Debug, Clone)]
pub enum AppMenuCmd {
    /// Прочитать меню окна `window`; `open` — какое верхнее меню после этого
    /// открыть (щёлкнули по нему), `None` — только названия.
    Load {
        window: u32,
        source: Source,
        open: Option<usize>,
    },
    /// Выбран пункт (id — из последнего прочитанного меню).
    Activate(i32),
}

#[derive(Debug, Clone)]
pub enum AppMenuUpdate {
    /// Меню окна: верхние пункты (название и пункты внутри); пусто — меню нет.
    Menu {
        window: u32,
        menus: Menus,
        open: Option<usize>,
    },
}

pub struct AppMenu(Sender<AppMenuCmd>);

impl AppMenu {
    pub fn send(&self, cmd: AppMenuCmd) {
        let _ = self.0.send(cmd);
    }
}

/// Что сделать по пункту меню.
enum Act {
    Gtk {
        bus: String,
        path: String,
        name: String,
        target: Option<OwnedValue>,
    },
}

pub fn start(notify: impl Fn(AppMenuUpdate) + Send + 'static) -> zbus::Result<AppMenu> {
    let conn = Connection::session()?;
    // Свой регистратор меню. Имя занято (регистратор vala-panel) — встаём в
    // очередь: он выключается, когда им не пользуются, и имя перейдёт к нам.
    let windows: Windows = Default::default();
    conn.object_server().at(
        registrar::PATH,
        Registrar {
            windows: windows.clone(),
        },
    )?;
    use zbus::fdo::RequestNameFlags;
    match conn.request_name_with_flags(registrar::NAME, RequestNameFlags::ReplaceExisting.into()) {
        Ok(reply) => eprintln!("Регистратор меню: {reply:?}"),
        Err(e) => eprintln!("Регистратор меню не занят: {e}"),
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Что делают пункты последнего прочитанного меню.
        let mut acts: HashMap<i32, Act> = HashMap::new();
        let mut dbusmenu: Option<(String, String)> = None;
        while let Ok(mut cmd) = rx.recv() {
            // Окна переключают быстро — читаем меню только последнего.
            while let Ok(next) = rx.try_recv() {
                match (&cmd, &next) {
                    (AppMenuCmd::Load { .. }, AppMenuCmd::Load { .. }) => cmd = next,
                    _ => {
                        handle(&conn, &windows, cmd, &mut acts, &mut dbusmenu, &notify);
                        cmd = next;
                    }
                }
            }
            handle(&conn, &windows, cmd, &mut acts, &mut dbusmenu, &notify);
        }
    });
    Ok(AppMenu(tx))
}

fn handle(
    conn: &Connection,
    windows: &Windows,
    cmd: AppMenuCmd,
    acts: &mut HashMap<i32, Act>,
    dbusmenu: &mut Option<(String, String)>,
    notify: &impl Fn(AppMenuUpdate),
) {
    match cmd {
        AppMenuCmd::Load {
            window,
            source,
            open,
        } => {
            acts.clear();
            *dbusmenu = None;
            let menus = match source {
                Source::Gtk(paths) => load_gtk(conn, &paths, acts).unwrap_or_default(),
                Source::Registrar => match load_registrar(conn, windows, window, open) {
                    Some((bus, path, menus)) => {
                        *dbusmenu = Some((bus, path));
                        menus
                    }
                    None => Vec::new(),
                },
            };
            notify(AppMenuUpdate::Menu {
                window,
                menus,
                open,
            });
        }
        AppMenuCmd::Activate(id) => {
            let result = match (acts.get(&id), dbusmenu.as_ref()) {
                (
                    Some(Act::Gtk {
                        bus,
                        path,
                        name,
                        target,
                    }),
                    _,
                ) => {
                    let params: Vec<Value> = target
                        .iter()
                        .filter_map(|t| t.try_clone().ok().map(Value::from))
                        .collect();
                    let platform: HashMap<&str, Value> = HashMap::new();
                    conn.call_method(
                        Some(bus.as_str()),
                        path.as_str(),
                        Some(GTK_ACTIONS),
                        "Activate",
                        &(name.as_str(), params, platform),
                    )
                    .map(|_| ())
                }
                (None, Some((bus, path))) => conn
                    .call_method(
                        Some(bus.as_str()),
                        path.as_str(),
                        Some(DBUSMENU),
                        "Event",
                        &(id, "clicked", Value::from(""), 0u32),
                    )
                    .map(|_| ()),
                (None, None) => Ok(()),
            };
            if let Err(e) = result {
                eprintln!("Глобальное меню: пункт не сработал: {e}");
            }
        }
    }
}

/// Свойства пункта меню GTK.
type GtkItem = HashMap<String, OwnedValue>;
/// Пункты меню GTK: группа → меню → пункты.
type GtkItems = HashMap<(u32, u32), Vec<GtkItem>>;
/// Верхние пункты меню окна: название и пункты внутри.
pub type Menus = Vec<(String, Vec<MenuItem>)>;

fn load_gtk(conn: &Connection, p: &GtkPaths, acts: &mut HashMap<i32, Act>) -> Option<Menus> {
    let (mut items, mut started) = fetch_gtk(conn, p, Vec::new())?;
    let mut states = describe_gtk(conn, p);
    // appmenu-gtk-module заполняет подменю, только когда их «открывают»:
    // у подменю есть действие-флажок (`submenu-action`). Ставим флажки и
    // читаем меню заново.
    let closed: Vec<String> = items
        .values()
        .flatten()
        .filter_map(|item| text(item, "submenu-action"))
        .filter(|a| {
            states.get(a).is_some_and(|(_, st)| {
                st.as_ref()
                    .and_then(|v| bool::try_from(v.try_clone().ok()?).ok())
                    == Some(false)
            })
        })
        .collect();
    if !closed.is_empty() {
        for action in &closed {
            set_state(conn, p, action, true);
        }
        if let Some((again, groups)) = fetch_gtk(conn, p, started.clone()) {
            items = again;
            started = groups;
        }
        states = describe_gtk(conn, p);
    }
    let _ = conn.call_method(
        Some(p.bus.as_str()),
        p.menubar.as_str(),
        Some(GTK_MENUS),
        "End",
        &(started,),
    );

    let mut next_id = 1;
    let mut build = Builder {
        items: &items,
        states: &states,
        paths: p,
        acts,
        next_id: &mut next_id,
    };
    // Верхние пункты бывают завёрнуты в разделы (так у appmenu-gtk-module:
    // (0,0) → (0,1) → (0,2) → «Файл», «Правка»…).
    let mut top = Vec::new();
    top_items(&items, (0, 0), 0, &mut top);
    let menus = top
        .into_iter()
        .filter_map(|item| {
            let label = strip_mnemonic(&text(item, "label")?);
            let (g, m) = link(item, ":submenu")?;
            Some((label, build.menu(g, m, 0)))
        })
        .collect();
    Some(menus)
}

/// Пункты меню `at`, с разделами, раскрытыми на месте.
fn top_items<'a>(items: &'a GtkItems, at: (u32, u32), depth: u32, out: &mut Vec<&'a GtkItem>) {
    let Some(list) = items.get(&at) else {
        return;
    };
    for item in list {
        match link(item, ":section") {
            Some(section) if depth < 8 => top_items(items, section, depth + 1, out),
            Some(_) => {}
            None => out.push(item),
        }
    }
}

/// Подписаться на группы меню, пока находятся новые ссылки (`:section`,
/// `:submenu` указывают на другие группы). `already` — уже подписанные
/// (их тоже перечитываем). Ответ: пункты и все подписанные группы.
fn fetch_gtk(conn: &Connection, p: &GtkPaths, already: Vec<u32>) -> Option<(GtkItems, Vec<u32>)> {
    let mut items: GtkItems = HashMap::new();
    let mut started: Vec<u32> = Vec::new();
    let mut want = if already.is_empty() {
        vec![0u32]
    } else {
        already
    };
    for _ in 0..16 {
        want.retain(|g| !started.contains(g));
        if want.is_empty() {
            break;
        }
        let reply = conn
            .call_method(
                Some(p.bus.as_str()),
                p.menubar.as_str(),
                Some(GTK_MENUS),
                "Start",
                &(want.clone(),),
            )
            .ok()?;
        started.extend(&want);
        let groups: Vec<(u32, u32, Vec<GtkItem>)> = reply.body().deserialize().ok()?;
        want.clear();
        for (g, m, list) in groups {
            for item in &list {
                for key in [":section", ":submenu"] {
                    if let Some((lg, _)) = link(item, key)
                        && !started.contains(&lg)
                        && !want.contains(&lg)
                    {
                        want.push(lg);
                    }
                }
            }
            items.insert((g, m), list);
        }
    }
    Some((items, started))
}

/// Действия программы, окна и модуля: доступно ли и состояние.
fn describe_gtk(conn: &Connection, p: &GtkPaths) -> HashMap<String, (bool, Option<OwnedValue>)> {
    let mut states = HashMap::new();
    for (prefix, path) in [("app", &p.app), ("win", &p.window), ("unity", &p.unity)] {
        let Some(path) = path else { continue };
        let Ok(reply) = conn.call_method(
            Some(p.bus.as_str()),
            path.as_str(),
            Some(GTK_ACTIONS),
            "DescribeAll",
            &(),
        ) else {
            continue;
        };
        let Ok(all) = reply
            .body()
            .deserialize::<HashMap<String, (bool, Signature, Vec<OwnedValue>)>>()
        else {
            continue;
        };
        for (name, (enabled, _, state)) in all {
            states.insert(
                format!("{prefix}.{name}"),
                (enabled, state.into_iter().next()),
            );
        }
    }
    states
}

/// Поставить состояние действия-флажка (`unity.имя`).
fn set_state(conn: &Connection, p: &GtkPaths, action: &str, value: bool) {
    let Some((prefix, name)) = action.split_once('.') else {
        return;
    };
    let path = match prefix {
        "app" => &p.app,
        "win" => &p.window,
        "unity" => &p.unity,
        _ => return,
    };
    let Some(path) = path else { return };
    let platform: HashMap<&str, Value> = HashMap::new();
    let _ = conn.call_method(
        Some(p.bus.as_str()),
        path.as_str(),
        Some(GTK_ACTIONS),
        "SetState",
        &(name, Value::from(value), platform),
    );
}

struct Builder<'a> {
    items: &'a GtkItems,
    states: &'a HashMap<String, (bool, Option<OwnedValue>)>,
    paths: &'a GtkPaths,
    acts: &'a mut HashMap<i32, Act>,
    next_id: &'a mut i32,
}

impl Builder<'_> {
    /// Пункты меню `(g, m)`; разделы — с разделителями вокруг.
    fn menu(&mut self, g: u32, m: u32, depth: u32) -> Vec<MenuItem> {
        let mut out = Vec::new();
        if depth > 8 {
            return out;
        }
        let Some(list) = self.items.get(&(g, m)) else {
            return out;
        };
        for item in list {
            if let Some((sg, sm)) = link(item, ":section") {
                let section = self.menu(sg, sm, depth + 1);
                if section.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(separator());
                }
                out.extend(section);
                out.push(separator());
                continue;
            }
            let label = strip_mnemonic(&text(item, "label").unwrap_or_default());
            if let Some((sg, sm)) = link(item, ":submenu") {
                let children = self.menu(sg, sm, depth + 1);
                out.push(MenuItem {
                    id: 0,
                    label,
                    enabled: true,
                    visible: true,
                    separator: false,
                    toggle: None,
                    children,
                });
                continue;
            }
            let action = text(item, "action").unwrap_or_default();
            let target = item.get("target").and_then(|t| t.try_clone().ok());
            let (enabled, toggle) = match self.states.get(&action) {
                Some((enabled, state)) => (*enabled, checked(state.as_ref(), target.as_ref())),
                // Действия нет — пункт неактивен (так у GTK).
                None => (false, None),
            };
            let id = *self.next_id;
            *self.next_id += 1;
            if let Some((prefix, name)) = action.split_once('.') {
                let path = match prefix {
                    "app" => self.paths.app.clone(),
                    "win" => self.paths.window.clone(),
                    "unity" => self.paths.unity.clone(),
                    _ => None,
                };
                if let Some(path) = path {
                    self.acts.insert(
                        id,
                        Act::Gtk {
                            bus: self.paths.bus.clone(),
                            path,
                            name: name.to_string(),
                            target,
                        },
                    );
                }
            }
            out.push(MenuItem {
                id,
                label,
                enabled,
                visible: true,
                separator: false,
                toggle,
                children: Vec::new(),
            });
        }
        // Разделители подряд и в конце не нужны.
        out.dedup_by(|a, b| a.separator && b.separator);
        if out.last().is_some_and(|i| i.separator) {
            out.pop();
        }
        if out.first().is_some_and(|i| i.separator) {
            out.remove(0);
        }
        out
    }
}

fn separator() -> MenuItem {
    MenuItem {
        id: 0,
        label: String::new(),
        enabled: true,
        visible: true,
        separator: true,
        toggle: None,
        children: Vec::new(),
    }
}

/// Галочка по состоянию действия: «да/нет» — как есть; для переключателя
/// (у пункта есть `target`) — совпадает ли состояние с ним.
fn checked(state: Option<&OwnedValue>, target: Option<&OwnedValue>) -> Option<bool> {
    let state = state?;
    if let Ok(b) = bool::try_from(state.try_clone().ok()?) {
        return Some(b);
    }
    let target = target?;
    Some(**state == **target)
}

fn text(item: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    String::try_from(item.get(key)?.try_clone().ok()?).ok()
}

fn link(item: &HashMap<String, OwnedValue>, key: &str) -> Option<(u32, u32)> {
    <(u32, u32)>::try_from(Value::from(item.get(key)?.try_clone().ok()?)).ok()
}

/// Меню через регистратор: где оно (имя на шине и путь) и верхние пункты.
fn load_registrar(
    conn: &Connection,
    windows: &Windows,
    window: u32,
    open: Option<usize>,
) -> Option<(String, String, Menus)> {
    // Сначала — у себя; нет — у регистратора на шине (если он не наш).
    let local = windows
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&window)
        .cloned();
    let (bus, path) = match local {
        Some(found) => found,
        None => {
            let reply = conn
                .call_method(
                    Some(REGISTRAR),
                    registrar::PATH,
                    Some(REGISTRAR),
                    "GetMenuForWindow",
                    &(window,),
                )
                .ok()?;
            let (bus, path): (String, zbus::zvariant::OwnedObjectPath) =
                reply.body().deserialize().ok()?;
            (bus, path.as_str().to_string())
        }
    };
    if bus.is_empty() {
        return None;
    }
    let layout = |id: i32| -> Option<Vec<MenuItem>> {
        let reply = conn
            .call_method(
                Some(bus.as_str()),
                path.as_str(),
                Some(DBUSMENU),
                "GetLayout",
                &(id, -1i32, Vec::<String>::new()),
            )
            .ok()?;
        let (_rev, root): (u32, Layout) = reply.body().deserialize().ok()?;
        Some(parse_layout(root))
    };
    let mut top = layout(0)?;
    // Qt достраивает подменю только перед показом.
    if let Some(i) = open
        && let Some(item) = top.get(i).map(|t| t.id)
    {
        let _ = conn.call_method(
            Some(bus.as_str()),
            path.as_str(),
            Some(DBUSMENU),
            "AboutToShow",
            &(item,),
        );
        if let Some(children) = layout(item) {
            top[i].children = children;
        }
    }
    let menus = top
        .into_iter()
        .filter(|t| t.visible && !t.separator)
        .map(|t| (t.label, t.children))
        .collect();
    Some((bus, path, menus))
}
