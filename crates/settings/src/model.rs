//! What the settings pages hold: rows with a label and a control, each
//! bound to a `section.key` of the config file, as `mimshel set` takes it.

use std::collections::HashMap;

use mimshel_core::config::Config;
use mimshel_core::i18n::{Language, t};

/// What a row lets you change.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// One of a list: `(value as mimshel set takes it, label)`.
    Choice(Vec<(String, String)>),
    /// On or off: the value written for each.
    Switch { on: String, off: String },
    /// A whole number in `min..=max`.
    Slider { min: u32, max: u32 },
    /// Free text, written on Return or when the field is left.
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// `section.key`, as in `mimshel set`.
    pub key: &'static str,
    pub label: String,
    /// A line of explanation under the row.
    pub hint: Option<String>,
    pub control: Control,
}

/// Rows shown together in one rounded box, with an optional title above.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub title: Option<String>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PageKind {
    Rows(Vec<Group>),
    /// Wallpaper thumbnails: `(value, label)`, then «Choose a picture…».
    Wallpaper(Vec<(String, String)>),
    /// Settings that live in another program's window: its command.
    Link(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub id: &'static str,
    pub title: String,
    /// Icon name in the theme.
    pub icon: &'static str,
    pub kind: PageKind,
}

/// What the pages offer that depends on the machine.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub gtk_themes: Vec<String>,
    pub icon_themes: Vec<String>,
    pub cursor_themes: Vec<String>,
    /// Clock formats to offer and how each looks right now.
    pub clocks: Vec<(String, String)>,
    /// The system pages whose program is installed (ids from [`LINKS`]).
    pub links: Vec<&'static str>,
}

/// System pages that open another program: id, icon, title key, command.
pub const LINKS: &[(&str, &str, &str, &[&str])] = &[
    (
        "displays",
        "preferences-desktop-display",
        "settings.displays",
        &["xfce4-display-settings"],
    ),
    (
        "keyboard",
        "preferences-desktop-keyboard",
        "settings.keyboard",
        &["xfce4-keyboard-settings"],
    ),
    (
        "mouse",
        "preferences-desktop-peripherals",
        "settings.mouse",
        &["xfce4-mouse-settings"],
    ),
    (
        "sound",
        "preferences-desktop-sound",
        "settings.sound",
        &["pavucontrol"],
    ),
    (
        "network",
        "preferences-system-network",
        "settings.network",
        &["nm-connection-editor"],
    ),
    (
        "power",
        "xfce4-power-manager-settings",
        "settings.power",
        &["xfce4-power-manager-settings"],
    ),
    (
        "bluetooth",
        "bluetooth",
        "settings.bluetooth",
        &["blueman-manager"],
    ),
    (
        "xfce",
        "org.xfce.settings.manager",
        "settings.xfce",
        &["xfce4-settings-manager"],
    ),
];

/// Every setting as `mimshel set` takes it (`"dock.icon_size" → "56"`),
/// defaults included: read back from the config written out in full.
pub fn values(config: &Config) -> HashMap<String, String> {
    use toml_edit::Value;
    let doc: toml_edit::DocumentMut = config.to_toml().parse().unwrap_or_default();
    let mut out = HashMap::new();
    for (section, item) in doc.iter() {
        let Some(table) = item.as_table() else {
            continue;
        };
        for (key, item) in table.iter() {
            let Some(value) = item.as_value() else {
                continue;
            };
            let sep = if (section, key) == ("apps", "terminal") {
                " "
            } else {
                ", "
            };
            let text = match value {
                Value::String(s) => s.value().clone(),
                Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(sep),
                other => other.to_string().trim().to_string(),
            };
            out.insert(format!("{section}.{key}"), text);
        }
    }
    out
}

/// All pages in `lang` for the current `values`.
pub fn pages(lang: Language, values: &HashMap<String, String>, env: &Env) -> Vec<Page> {
    let tr = |key: &str| t(lang, key).to_string();
    let value = |key: &str| values.get(key).map_or("", String::as_str);
    let row = |key: &'static str, label: &str, control: Control| Row {
        key,
        label: tr(label),
        hint: None,
        control,
    };
    let choice = |key: &'static str, label: &str, options: Vec<(String, String)>| {
        row(
            key,
            label,
            Control::Choice(with_current(options, value(key))),
        )
    };
    let names = |list: &[String]| list.iter().map(|n| (n.clone(), n.clone())).collect();
    let minutes = |key: &'static str, label: &str| {
        let options = [1, 2, 3, 5, 10, 15, 20, 30, 45, 60]
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    tr("settings.minutes").replace("{n}", &n.to_string()),
                )
            })
            .collect();
        choice(key, label, options)
    };
    let switch = |key: &'static str, label: &str, on: &str, off: &str| {
        row(
            key,
            label,
            Control::Switch {
                on: on.to_string(),
                off: off.to_string(),
            },
        )
    };
    let group = |rows: Vec<Row>| Group { title: None, rows };

    let languages = vec![
        ("auto".to_string(), tr("settings.language_auto")),
        ("en".to_string(), "English".to_string()),
        ("ru".to_string(), "Русский".to_string()),
        ("zh".to_string(), "简体中文".to_string()),
        ("zh_TW".to_string(), "繁體中文".to_string()),
    ];
    let mut scales = vec![("auto".to_string(), tr("settings.scale_auto"))];
    for s in ["1", "1.25", "1.5", "1.75", "2", "2.5", "3"] {
        let percent = (s.parse::<f32>().unwrap_or(1.0) * 100.0).round();
        scales.push((s.to_string(), format!("{percent} %")));
    }
    let mut png = switch("look.png_icons", "settings.png_icons", "true", "false");
    png.hint = Some(tr("settings.png_icons_hint"));
    let appearance = vec![
        group(vec![
            choice("look.language", "settings.language", languages),
            choice("look.scale", "settings.scale", scales),
        ]),
        group(vec![
            choice(
                "look.gtk_theme",
                "settings.gtk_theme",
                names(&env.gtk_themes),
            ),
            choice(
                "look.icon_theme",
                "settings.icon_theme",
                names(&env.icon_themes),
            ),
            choice(
                "look.cursor_theme",
                "settings.cursor_theme",
                names(&env.cursor_themes),
            ),
            row("look.font", "settings.font", Control::Text),
        ]),
        group(vec![png]),
    ];

    let wallpapers = ["space", "waves", "aurora"]
        .iter()
        .map(|w| (w.to_string(), tr(&format!("settings.wallpaper_{w}"))))
        .collect::<Vec<_>>();
    let mut wallpapers = wallpapers;
    let current = value("look.wallpaper");
    if !current.is_empty() && !wallpapers.iter().any(|(v, _)| v == current) {
        wallpapers.push((current.to_string(), tr("settings.wallpaper_picture")));
    }

    let mut menu_icon = row("menubar.menu_icon", "settings.menu_icon", Control::Text);
    menu_icon.hint = Some(tr("settings.menu_icon_hint"));
    let menubar = vec![group(vec![
        row(
            "menubar.height",
            "settings.menubar_height",
            Control::Slider { min: 20, max: 64 },
        ),
        choice(
            "menubar.clock_format",
            "settings.clock_format",
            env.clocks.clone(),
        ),
        menu_icon,
    ])];

    let zoom = value("dock.zoom");
    let zoom_on = zoom != "false" && !zoom.is_empty();
    let mut magnify = vec![switch(
        "dock.zoom",
        "settings.magnify",
        if zoom_on { zoom } else { "140" },
        "false",
    )];
    if zoom_on {
        magnify.push(row(
            "dock.zoom",
            "settings.magnify_amount",
            Control::Slider { min: 110, max: 300 },
        ));
    }
    let hide = ["never", "smart", "auto", "maximized"]
        .iter()
        .map(|h| (h.to_string(), tr(&format!("settings.hide_{h}"))))
        .collect();
    let dock = vec![
        group(vec![row(
            "dock.icon_size",
            "settings.icon_size",
            Control::Slider { min: 24, max: 128 },
        )]),
        group(magnify),
        group(vec![choice("dock.hide", "settings.dock_hide", hide)]),
    ];

    let lock = vec![
        group(vec![
            minutes("lock.idle_minutes", "settings.lock_after"),
            minutes("lock.screen_off_minutes", "settings.screen_off"),
        ]),
        group(vec![row(
            "lock.placeholder",
            "settings.placeholder",
            Control::Text,
        )]),
    ];

    let apps = vec![Group {
        title: Some(tr("settings.apps_window")),
        rows: vec![
            row(
                "apps_panel.columns",
                "settings.columns",
                Control::Slider { min: 3, max: 12 },
            ),
            row(
                "apps_panel.rows",
                "settings.rows",
                Control::Slider { min: 2, max: 10 },
            ),
            row(
                "apps_panel.icon_size",
                "settings.icon_size",
                Control::Slider { min: 48, max: 192 },
            ),
        ],
    }];

    let terminal = vec![group(vec![
        row("apps.terminal", "settings.terminal_command", Control::Text),
        switch("terminal.osc52", "settings.osc52", "allow", "off"),
    ])];

    let parts = vec![Group {
        title: Some(tr("settings.parts_hint")),
        rows: vec![
            switch("parts.dock", "settings.part_dock", "mimshel", "plank"),
            switch("parts.menubar", "settings.part_menubar", "mimshel", "xfce"),
            switch("parts.notify", "settings.part_notify", "mimshel", "xfce"),
            switch("parts.desktop", "settings.part_desktop", "mimshel", "xfce"),
            switch(
                "parts.folders",
                "settings.part_folders",
                "mimshel",
                "thunar",
            ),
            switch(
                "parts.search_panel",
                "settings.part_search",
                "mimshel",
                "rofi",
            ),
            switch("parts.apps_panel", "settings.part_apps", "mimshel", "rofi"),
        ],
    }];

    let page = |id, icon, title: &str, kind| Page {
        id,
        title: tr(title),
        icon,
        kind,
    };
    let mut out = vec![
        page(
            "appearance",
            "preferences-desktop-theme",
            "settings.appearance",
            PageKind::Rows(appearance),
        ),
        page(
            "wallpaper",
            "preferences-desktop-wallpaper",
            "settings.wallpaper",
            PageKind::Wallpaper(wallpapers),
        ),
        page(
            "menubar",
            "preferences-desktop",
            "settings.menubar",
            PageKind::Rows(menubar),
        ),
        page(
            "dock",
            "preferences-system-windows",
            "settings.dock",
            PageKind::Rows(dock),
        ),
        page(
            "lock",
            "system-lock-screen",
            "settings.lock",
            PageKind::Rows(lock),
        ),
        page(
            "apps",
            "applications-other",
            "settings.apps",
            PageKind::Rows(apps),
        ),
        page(
            "terminal",
            "utilities-terminal",
            "settings.terminal",
            PageKind::Rows(terminal),
        ),
        page(
            "parts",
            "preferences-other",
            "settings.parts",
            PageKind::Rows(parts),
        ),
    ];
    for (id, icon, title, command) in LINKS {
        if env.links.contains(id) {
            let command = command.iter().map(|s| s.to_string()).collect();
            out.push(page(id, icon, title, PageKind::Link(command)));
        }
    }
    out
}

/// `options` with the current value in it: a value set by hand (a theme
/// that is not installed, scale 1.3) is shown as it is, not as another one.
fn with_current(mut options: Vec<(String, String)>, current: &str) -> Vec<(String, String)> {
    if !current.is_empty() && !options.iter().any(|(v, _)| v == current) {
        options.push((current.to_string(), current.to_string()));
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_hold_every_setting_as_set_takes_it() {
        let v = values(&Config::default());
        assert_eq!(v["dock.icon_size"], Config::default().dock_icon.to_string());
        assert_eq!(v["dock.hide"], "smart");
        assert_eq!(v["look.language"], "en");
        assert_eq!(
            v["parts.dock"],
            if Config::default().dock {
                "mimshel"
            } else {
                "plank"
            }
        );
        assert!(!v["apps.terminal"].contains(','), "{}", v["apps.terminal"]);
        // Every value can be written back by `mimshel set`.
        for (key, _) in mimshel_core::config::SETTINGS {
            let Some(value) = v.get(*key) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            assert!(
                mimshel_core::config::setting_value(key, value).is_ok(),
                "{key} = {value}"
            );
        }
    }

    #[test]
    fn every_row_is_a_known_setting() {
        let v = values(&Config::default());
        let env = Env {
            links: LINKS.iter().map(|l| l.0).collect(),
            ..Env::default()
        };
        let all = pages(Language::Ru, &v, &env);
        for page in &all {
            if let PageKind::Rows(groups) = &page.kind {
                for row in groups.iter().flat_map(|g| &g.rows) {
                    assert!(
                        mimshel_core::config::SETTINGS
                            .iter()
                            .any(|(k, _)| k == &row.key),
                        "{}",
                        row.key
                    );
                    assert!(!row.label.starts_with("settings."), "{}", row.label);
                }
            }
        }
        assert_eq!(all.len(), 8 + LINKS.len());
    }

    #[test]
    fn current_values_stay_in_choices() {
        let mut v = values(&Config::default());
        v.insert("look.scale".into(), "1.3".into());
        let all = pages(Language::En, &v, &Env::default());
        let PageKind::Rows(groups) = &all[0].kind else {
            panic!("appearance has rows");
        };
        let scale = &groups[0].rows[1];
        let Control::Choice(options) = &scale.control else {
            panic!("scale is a choice");
        };
        assert!(options.iter().any(|(v, _)| v == "1.3"));
        assert!(options.iter().any(|(v, l)| v == "1.5" && l == "150 %"));
    }

    #[test]
    fn zoom_amount_only_when_zoom_is_on() {
        let mut v = values(&Config::default());
        let dock = |v: &HashMap<String, String>| {
            let all = pages(Language::En, v, &Env::default());
            match &all[3].kind {
                PageKind::Rows(groups) => groups[1].rows.len(),
                _ => 0,
            }
        };
        v.insert("dock.zoom".into(), "180".into());
        assert_eq!(dock(&v), 2);
        v.insert("dock.zoom".into(), "false".into());
        assert_eq!(dock(&v), 1);
    }
}
