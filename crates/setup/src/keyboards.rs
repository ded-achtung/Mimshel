//! XKB layouts and Xfce keyboard preferences, without restarting session services.

use crate::sys;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    pub layouts: Vec<(String, String)>,
    pub switch: Option<String>,
    pub other_options: Vec<String>,
    pub delay: u32,
    pub rate: u32,
    pub numlock: Option<bool>,
}

pub fn parse_setxkbmap(query: &str) -> (Vec<(String, String)>, Vec<String>) {
    let field = |key| {
        query
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then_some(value.trim())
            })
            .unwrap_or("")
    };
    let variants: Vec<_> = field("variant").split(',').collect();
    let layouts = field("layout")
        .split(',')
        .enumerate()
        .filter(|(_, layout)| !layout.trim().is_empty())
        .map(|(i, layout)| {
            (
                layout.trim().into(),
                variants.get(i).unwrap_or(&"").trim().into(),
            )
        })
        .collect();
    let options = field("options")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (layouts, options)
}

pub fn parse_repeat(xset_q: &str) -> Option<(u32, u32)> {
    let line = xset_q.lines().find(|l| l.contains("auto repeat delay:"))?;
    let (delay, rate) = line
        .split_once("auto repeat delay:")?
        .1
        .split_once("repeat rate:")?;
    Some((delay.trim().parse().ok()?, rate.trim().parse().ok()?))
}

pub fn current() -> Keyboard {
    // Read the server, not the potentially empty keyboard-layout channel. The
    // system X11 configuration supplies layouts when Xfce uses system defaults.
    let (mut layouts, options) =
        parse_setxkbmap(&sys::output(&["setxkbmap", "-query"]).unwrap_or_default());
    if layouts.is_empty() {
        // A failed query must not present an empty editable layout list.
        layouts.push(("us".into(), String::new()));
    }
    let (delay, rate) = sys::output(&["xset", "q"])
        .and_then(|s| parse_repeat(&s))
        .unwrap_or((500, 20));
    let switch = options.iter().find(|o| o.starts_with("grp:")).cloned();
    let other_options = options
        .into_iter()
        .filter(|o| !o.starts_with("grp:"))
        .collect();
    let numlock = sys::xget("keyboards", "/Default/Numlock").and_then(|s| s.parse().ok());
    Keyboard {
        layouts,
        switch,
        other_options,
        delay,
        rate,
        numlock,
    }
}

pub fn parse_layout_list(evdev_lst: &str) -> Vec<(String, String)> {
    let mut in_layouts = false;
    let mut layouts = Vec::new();
    for line in evdev_lst.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix('!') {
            in_layouts = section.trim() == "layout";
        } else if in_layouts && let Some(i) = line.find(char::is_whitespace) {
            let (code, description) = line.split_at(i);
            if !description.trim().is_empty() {
                layouts.push((code.into(), description.trim().into()));
            }
        }
    }
    layouts
}

pub fn available_layouts() -> Vec<(String, String)> {
    parse_layout_list(&sys::read(std::path::Path::new(
        "/usr/share/X11/xkb/rules/evdev.lst",
    )))
}

pub const SWITCHES: &[&str] = &[
    "grp:alt_shift_toggle",
    "grp:ctrl_shift_toggle",
    "grp:win_space_toggle",
    "grp:caps_toggle",
    "grp:menu_toggle",
    "grp:lwin_toggle",
    "grp:rwin_toggle",
];

fn layout_strings(k: &Keyboard) -> (String, String) {
    (
        k.layouts
            .iter()
            .map(|(l, _)| l.as_str())
            .collect::<Vec<_>>()
            .join(","),
        k.layouts
            .iter()
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join(","),
    )
}

fn options(k: &Keyboard) -> Vec<&str> {
    k.other_options
        .iter()
        .map(String::as_str)
        .chain(k.switch.as_deref())
        .collect()
}

// Xfce 4.20, commit d56c7b38b48cf639f2aecd4acc7bcada24c1f478:
// xfsettingsd/keyboard-layout.c:116 (XkbDisable: bool), 244-252,265,273
// (XkbLayout/XkbVariant: comma-separated strings), 338,354-361,376,382
// (XkbOptions/Group and Compose: strings).
// There is no generic XkbOptions property. The Group reader inserts its whole
// string into the options list and splits on commas (354-361), so its payload
// also carries non-compose options across login. Compose has its own reader.
pub fn layout_props(k: &Keyboard) -> Vec<(String, &'static str, String)> {
    let (layouts, variants) = layout_strings(k);
    let all_options = options(k);
    let group = all_options
        .iter()
        .copied()
        .filter(|o| !o.starts_with("compose:"))
        .collect::<Vec<_>>()
        .join(",");
    let compose = all_options
        .iter()
        .copied()
        .filter(|o| o.starts_with("compose:"))
        .collect::<Vec<_>>()
        .join(",");
    vec![
        ("/Default/XkbLayout".into(), "string", layouts),
        ("/Default/XkbVariant".into(), "string", variants),
        ("/Default/XkbOptions/Group".into(), "string", group),
        ("/Default/XkbOptions/Compose".into(), "string", compose),
        ("/Default/XkbDisable".into(), "bool", "false".into()),
    ]
}

fn validate_layouts(k: &Keyboard) -> Result<(), String> {
    if k.layouts.is_empty() || k.layouts.iter().any(|(l, _)| l.trim().is_empty()) {
        return Err("Должна остаться хотя бы одна непустая раскладка".into());
    }
    Ok(())
}

pub fn set_layouts(k: &Keyboard) -> Result<(), String> {
    validate_layouts(k)?;
    // Stage the complete profile while disabled; never activate a partial list
    // or reset the channel (lessons 9,15,18). No model or rules are overwritten.
    write_props(
        "keyboard-layout",
        vec![("/Default/XkbDisable".into(), "bool", "true".into())],
    )?;
    write_props("keyboard-layout", layout_props(k))?;
    // xfsettingsd applies changes immediately with libxklavier:
    // keyboard-layout.c:127,394-421,252,361. Its option readers retain other
    // server options; explicitly replace the full option list so removals work
    // too. One complete setxkbmap request also updates the core/XTEST keyboard.
    let result = sys::command("setxkbmap")
        .args(setxkbmap_args(k))
        .output()
        .map_err(|e| format!("Не удалось запустить setxkbmap: {e}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Не удалось настроить раскладки: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

fn setxkbmap_args(k: &Keyboard) -> Vec<String> {
    let (layouts, variants) = layout_strings(k);
    vec![
        "-layout".into(),
        layouts,
        "-variant".into(),
        variants,
        "-option".into(),
        String::new(),
        "-option".into(),
        options(k).join(","),
    ]
}

// xfsettingsd/keyboards.c:178 (KeyRepeat: bool),200-201 (Delay/Rate: int).
// The property-changed handler at 241-250 calls XkbSetControls (217) at once.
pub fn repeat_props(delay: u32, rate: u32) -> Vec<(String, &'static str, String)> {
    vec![
        ("/Default/KeyRepeat/Delay".into(), "int", delay.to_string()),
        ("/Default/KeyRepeat/Rate".into(), "int", rate.to_string()),
        ("/Default/KeyRepeat".into(), "bool", "true".into()),
    ]
}

pub fn set_repeat(delay: u32, rate: u32) -> Result<(), String> {
    if delay == 0 || delay > u16::MAX.into() || rate == 0 || rate > 1000 {
        return Err("Недопустимые задержка или частота повтора клавиш".into());
    }
    write_props("keyboards", repeat_props(delay, rate))
}

pub fn set_numlock(on: bool) -> Result<(), String> {
    // xfsettingsd/keyboards.c:263-266: Numlock and RestoreNumlock are bools.
    // RestoreNumlock must be enabled for the saved state to apply at login.
    write_props(
        "keyboards",
        vec![
            ("/Default/Numlock".into(), "bool", on.to_string()),
            ("/Default/RestoreNumlock".into(), "bool", "true".into()),
        ],
    )
}

fn write_props(channel: &str, props: Vec<(String, &'static str, String)>) -> Result<(), String> {
    for (property, kind, value) in props {
        if !sys::xset(channel, &property, kind, &value) {
            return Err(format!(
                "Не удалось сохранить настройку клавиатуры: {property}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyboard() -> Keyboard {
        Keyboard {
            layouts: vec![("us".into(), "".into()), ("ru".into(), "".into())],
            switch: Some("grp:alt_shift_toggle".into()),
            other_options: vec!["caps:escape".into(), "compose:ralt".into()],
            delay: 500,
            rate: 30,
            numlock: None,
        }
    }

    #[test]
    fn system_layout_query_without_variants() {
        let (layouts, options) = parse_setxkbmap(
            "rules:      evdev\nmodel:      pc105\nlayout:     us,ru\noptions:    grp:alt_shift_toggle\n",
        );
        assert_eq!(
            layouts,
            [("us".into(), "".into()), ("ru".into(), "".into())]
        );
        assert_eq!(options, ["grp:alt_shift_toggle"]);
    }

    #[test]
    fn variants_preserve_empty_slots_and_options() {
        let (layouts, options) = parse_setxkbmap(
            "layout: us,ru,de\nvariant: ,phonetic,\noptions: caps:escape, grp:win_space_toggle,compose:ralt\n",
        );
        assert_eq!(
            layouts,
            [
                ("us".into(), "".into()),
                ("ru".into(), "phonetic".into()),
                ("de".into(), "".into())
            ]
        );
        assert_eq!(
            options,
            ["caps:escape", "grp:win_space_toggle", "compose:ralt"]
        );
    }

    #[test]
    fn missing_or_short_fields() {
        assert_eq!(parse_setxkbmap(""), (vec![], vec![]));
        let (layouts, options) = parse_setxkbmap("layout: us,ru\nvariant: intl\noptions:\n");
        assert_eq!(layouts[1], ("ru".into(), "".into()));
        assert!(options.is_empty());
    }

    #[test]
    fn repeat_from_xset_text() {
        let text = "Keyboard Control:\n  auto repeat:  on    key click percent:  0    LED mask:  00000000\n  XKB indicators:\n    00: Caps Lock:   off    01: Num Lock:    off\n  auto repeat delay:  660    repeat rate:  25\n  auto repeating keys:  00ffffffdffffbbf\nPointer Control:\n  acceleration:  2/1    threshold:  4\n";
        assert_eq!(parse_repeat(text), Some((660, 25)));
        assert_eq!(
            parse_repeat("auto repeat delay: 0 repeat rate: 0"),
            Some((0, 0))
        );
    }

    #[test]
    fn repeat_rejects_incomplete_text() {
        assert_eq!(parse_repeat(""), None);
        assert_eq!(parse_repeat("auto repeat delay: 500"), None);
        assert_eq!(parse_repeat("auto repeat delay: bad repeat rate: 20"), None);
    }

    #[test]
    fn layout_section_only() {
        let list = "! model\n  pc105 Generic 105-key PC\n\n! layout\n  us English (US)\n  ru Russian\n  de German\n\n! variant\n  phonetic ru: Russian (phonetic)\n! option\n  grp:alt_shift_toggle Alt+Shift\n";
        assert_eq!(
            parse_layout_list(list),
            [
                ("us".into(), "English (US)".into()),
                ("ru".into(), "Russian".into()),
                ("de".into(), "German".into())
            ]
        );
        assert!(parse_layout_list("! model\n pc105 Generic\n").is_empty());
    }

    #[test]
    fn switches_exist_in_evdev_sample() {
        // Option codes and descriptions from xkeyboard-config rules/evdev.lst.
        let sample = "! option\n  grp:alt_shift_toggle Alt+Shift\n  grp:ctrl_shift_toggle Ctrl+Shift\n  grp:win_space_toggle Win+Space\n  grp:caps_toggle Caps Lock\n  grp:menu_toggle Menu\n  grp:lwin_toggle Left Win\n  grp:rwin_toggle Right Win\n";
        let codes: Vec<_> = sample
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .collect();
        for switch in SWITCHES {
            assert!(codes.contains(switch), "{switch}");
        }
    }

    #[test]
    fn layout_properties_include_aligned_variants_and_all_options() {
        assert_eq!(
            layout_props(&keyboard()),
            [
                ("/Default/XkbLayout".into(), "string", "us,ru".into()),
                ("/Default/XkbVariant".into(), "string", ",".into()),
                (
                    "/Default/XkbOptions/Group".into(),
                    "string",
                    "caps:escape,grp:alt_shift_toggle".into()
                ),
                (
                    "/Default/XkbOptions/Compose".into(),
                    "string",
                    "compose:ralt".into()
                ),
                ("/Default/XkbDisable".into(), "bool", "false".into()),
            ]
        );
    }

    #[test]
    fn no_switch_or_options_and_single_variant() {
        let mut k = keyboard();
        k.layouts = vec![("us".into(), "intl".into())];
        k.switch = None;
        k.other_options.clear();
        let props = layout_props(&k);
        assert_eq!(props[1].2, "intl");
        assert_eq!(props[2].2, "");
        assert_eq!(props[3].2, "");
    }

    #[test]
    fn complete_request_clears_old_options_without_losing_variants() {
        let mut k = keyboard();
        k.layouts[1].1 = "phonetic".into();
        assert_eq!(
            setxkbmap_args(&k),
            [
                "-layout",
                "us,ru",
                "-variant",
                ",phonetic",
                "-option",
                "",
                "-option",
                "caps:escape,compose:ralt,grp:alt_shift_toggle",
            ]
        );
        k.switch = None;
        k.other_options.clear();
        assert_eq!(setxkbmap_args(&k).last().unwrap(), "");
    }

    #[test]
    fn empty_layouts_refused_before_any_command() {
        let mut k = keyboard();
        k.layouts.clear();
        assert!(set_layouts(&k).is_err());
        k.layouts.push((" ".into(), "".into()));
        assert!(set_layouts(&k).is_err());
    }

    #[test]
    fn repeat_properties_have_xfce_types() {
        assert_eq!(
            repeat_props(660, 25),
            [
                ("/Default/KeyRepeat/Delay".into(), "int", "660".into()),
                ("/Default/KeyRepeat/Rate".into(), "int", "25".into()),
                ("/Default/KeyRepeat".into(), "bool", "true".into()),
            ]
        );
    }

    #[test]
    fn invalid_repeat_refused_before_any_command() {
        for (delay, rate) in [(0, 20), (u32::MAX, 20), (500, 0), (500, 1001)] {
            assert!(set_repeat(delay, rate).is_err());
        }
    }
}
