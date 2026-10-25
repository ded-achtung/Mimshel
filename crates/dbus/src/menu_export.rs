//! A window's own menu in the top menu bar, like macOS: the window publishes
//! its items via DBusMenu (`com.canonical.dbusmenu`) and tells the registrar
//! (`com.canonical.AppMenu.Registrar`, owned by the Mimshel service) where they are.
//! The menu bar reads them like any other program's menu, and a click on an
//! item comes back here — to `on_click` with the item id.
//!
//! For our own windows that have no menu at all: the terminal (later the file
//! manager and others). Items are fixed; the window decides what a click does.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zbus::interface;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

/// Where the menu lives on the bus.
const PATH: &str = "/MenuBar";

/// A menu item: id (sent back on click), label, separator,
/// submenu. Id 0 is taken by the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSpec {
    pub id: i32,
    pub label: String,
    pub separator: bool,
    pub children: Vec<MenuSpec>,
}

impl MenuSpec {
    pub fn item(id: i32, label: &str) -> MenuSpec {
        MenuSpec {
            id,
            label: label.to_string(),
            separator: false,
            children: Vec::new(),
        }
    }

    /// A separator (needs its own unused id).
    pub fn separator(id: i32) -> MenuSpec {
        MenuSpec {
            separator: true,
            ..MenuSpec::item(id, "")
        }
    }

    /// A top-level menu with items («Правка» → «Копировать», «Вставить»…).
    pub fn menu(id: i32, label: &str, children: Vec<MenuSpec>) -> MenuSpec {
        MenuSpec {
            children,
            ..MenuSpec::item(id, label)
        }
    }

    fn find(&self, id: i32) -> Option<&MenuSpec> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
}

type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn props(item: &MenuSpec) -> HashMap<String, OwnedValue> {
    let mut p = HashMap::new();
    let mut put = |k: &str, v: Value<'_>| {
        if let Ok(v) = OwnedValue::try_from(v) {
            p.insert(k.to_string(), v);
        }
    };
    if item.separator {
        put("type", Value::from("separator"));
    } else {
        put("label", Value::from(item.label.as_str()));
    }
    if !item.children.is_empty() {
        put("children-display", Value::from("submenu"));
    }
    p
}

/// Node `(id, properties, children)`; each child wrapped in a variant, depth
/// `depth` (-1 — all).
fn node(item: &MenuSpec, depth: i32) -> Node {
    let children = if depth == 0 {
        Vec::new()
    } else {
        item.children
            .iter()
            .filter_map(|c| OwnedValue::try_from(Value::from(node(c, depth - 1))).ok())
            .collect()
    };
    (item.id, props(item), children)
}

struct DbusMenu {
    root: MenuSpec,
    on_click: Box<dyn Fn(i32) + Send + Sync>,
}

#[interface(name = "com.canonical.dbusmenu")]
impl DbusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> (u32, Node) {
        let item = self.root.find(parent_id).unwrap_or(&self.root);
        (1, node(item, recursion_depth))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        ids.into_iter()
            .filter_map(|id| self.root.find(id).map(|i| (id, props(i))))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> OwnedValue {
        self.root
            .find(id)
            .and_then(|i| props(i).remove(&name))
            .unwrap_or_else(|| OwnedValue::from(0i32))
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        if event_id == "clicked" {
            (self.on_click)(id);
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, _, _) in events {
            if event_id == "clicked" {
                (self.on_click)(id);
            }
        }
        Vec::new()
    }

    /// Items are fixed — nothing to build before showing.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> String {
        "ltr".into()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "normal".into()
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The menu is on the bus: it stays visible while this lives.
pub struct ExportedMenu {
    _conn: Connection,
}

/// Publish `menus` (top-level items) for X11 window `window` and tell the
/// registrar. A click calls `on_click(id)` (from the bus thread).
pub fn export(
    window: u32,
    menus: Vec<MenuSpec>,
    on_click: impl Fn(i32) + Send + Sync + 'static,
) -> zbus::Result<ExportedMenu> {
    let conn = Connection::session()?;
    let root = MenuSpec::menu(0, "", menus);
    conn.object_server().at(
        PATH,
        DbusMenu {
            root,
            on_click: Box::new(on_click),
        },
    )?;
    conn.call_method(
        Some(crate::registrar::NAME),
        crate::registrar::PATH,
        Some(crate::registrar::NAME),
        "RegisterWindow",
        &(window, ObjectPath::try_from(PATH)?),
    )?;
    Ok(ExportedMenu { _conn: conn })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::parse_layout;

    fn menus() -> MenuSpec {
        MenuSpec::menu(
            0,
            "",
            vec![
                MenuSpec::menu(
                    1,
                    "Правка",
                    vec![
                        MenuSpec::item(10, "Копировать"),
                        MenuSpec::separator(11),
                        MenuSpec::item(12, "Вставить"),
                    ],
                ),
                MenuSpec::menu(2, "Вид", vec![MenuSpec::item(20, "Крупнее")]),
            ],
        )
    }

    /// What we publish is read back the same way by the menu bar (its `parse_layout`).
    #[test]
    fn layout_reads_back() {
        let root = menus();
        let (_, props, children) = node(&root, -1);
        assert!(props.contains_key("children-display"));
        // As over the bus: children are variants; parsed by the same function as other programs' menus.
        let items = parse_layout((0, HashMap::new(), children));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Правка");
        let edit: Vec<(&str, bool)> = items[0]
            .children
            .iter()
            .map(|i| (i.label.as_str(), i.separator))
            .collect();
        assert_eq!(
            edit,
            [("Копировать", false), ("", true), ("Вставить", false)]
        );
        assert_eq!(items[1].children[0].id, 20);
        // Top level only (depth 1): menus have no children.
        let (_, _, shallow) = node(&root, 1);
        let top = parse_layout((0, HashMap::new(), shallow));
        assert!(top[0].children.is_empty());
        assert_eq!(root.find(12).map(|i| i.label.as_str()), Some("Вставить"));
    }
}
