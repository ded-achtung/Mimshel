//! Меню значка трея по стандарту DBusMenu (`com.canonical.dbusmenu`):
//! дерево пунктов `(id, свойства, дети)`. Разбираем то, что нужно для
//! показа: название (без подчёркиваний-«мнемоник»), доступность, видимость,
//! разделитель, галочка, подменю.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub separator: bool,
    /// Галочка (или переключатель): `Some(включено)`; `None` — без галочки.
    pub toggle: Option<bool>,
    /// Пункты подменю.
    pub children: Vec<MenuItem>,
}

/// Узел меню, как его присылает `GetLayout`: id, свойства, дети (каждый —
/// «вариант» с таким же узлом внутри).
pub type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// Пункты меню (дети корня) из ответа `GetLayout`.
pub fn parse_layout(root: Layout) -> Vec<MenuItem> {
    node(root).children
}

fn parse_node(v: Value<'_>) -> Option<MenuItem> {
    // Ребёнок приходит завёрнутым в «вариант» ещё раз.
    let v = match v {
        Value::Value(inner) => *inner,
        other => other,
    };
    Layout::try_from(v).ok().map(node)
}

fn node((id, props, children): Layout) -> MenuItem {
    let text = |k: &str| {
        props
            .get(k)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
    };
    let flag = |k: &str| {
        props
            .get(k)
            .and_then(|v| bool::try_from(v.try_clone().ok()?).ok())
    };
    let toggle = text("toggle-type")
        .filter(|t| t == "checkmark" || t == "radio")
        .map(|_| {
            props
                .get("toggle-state")
                .and_then(|v| i32::try_from(v.try_clone().ok()?).ok())
                == Some(1)
        });
    MenuItem {
        id,
        label: strip_mnemonic(&text("label").unwrap_or_default()),
        enabled: flag("enabled").unwrap_or(true),
        visible: flag("visible").unwrap_or(true),
        separator: text("type").as_deref() == Some("separator"),
        toggle,
        children: children
            .into_iter()
            .filter_map(|c| parse_node(Value::from(c)))
            .collect(),
    }
}

/// «_Файл» → «Файл», «a__b» → «a_b» (подчёркивание — клавиша-подсказка).
fn strip_mnemonic(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                out.push('_');
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::StructureBuilder;

    fn node(
        id: i32,
        props: Vec<(&str, Value<'static>)>,
        children: Vec<Value<'static>>,
    ) -> Value<'static> {
        let map: HashMap<String, Value<'static>> =
            props.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let kids: Vec<Value<'static>> = children
            .into_iter()
            .map(|c| Value::Value(Box::new(c)))
            .collect();
        StructureBuilder::new()
            .add_field(id)
            .add_field(map)
            .add_field(kids)
            .build()
            .unwrap()
            .into()
    }

    #[test]
    fn parses_tree() {
        let wifi = node(
            2,
            vec![
                ("label", Value::from("_Включить Wi-Fi")),
                ("toggle-type", Value::from("checkmark")),
                ("toggle-state", Value::from(1i32)),
            ],
            vec![],
        );
        let sep = node(3, vec![("type", Value::from("separator"))], vec![]);
        let sub = node(
            4,
            vec![
                ("label", Value::from("VPN")),
                ("children-display", Value::from("submenu")),
            ],
            vec![node(
                5,
                vec![
                    ("label", Value::from("a__b")),
                    ("enabled", Value::from(false)),
                ],
                vec![],
            )],
        );
        let root = node(0, vec![], vec![wifi, sep, sub]);
        let items = parse_layout(Layout::try_from(root).unwrap());
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].label, "Включить Wi-Fi");
        assert_eq!(items[0].toggle, Some(true));
        assert!(items[1].separator);
        assert_eq!(items[2].children[0].label, "a_b");
        assert!(!items[2].children[0].enabled);
        assert!(items[2].children[0].visible);
    }
}
