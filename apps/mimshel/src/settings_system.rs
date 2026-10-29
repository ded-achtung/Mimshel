//! The Displays and Keyboard pages of the settings window: what they show
//! and what a change does, on top of `setup::displays` and
//! `setup::keyboards`. Their rows have keys of their own —
//! `display.NAME.WHAT` and `keyboard.WHAT` — that never reach the config
//! file.

use std::collections::HashMap;

use mimshel_core::i18n::{Language, t};
use mimshel_settings::Monitor;
use mimshel_setup::displays::{self, Output, Rotation, Side};
use mimshel_setup::keyboards::{self, Keyboard};

/// A row of the Displays or Keyboard page (not a config setting).
pub fn is_system(key: &str) -> bool {
    key.starts_with("display.") || key.starts_with("keyboard.")
}

/// A refresh rate as the page offers it.
fn rate(r: f64) -> String {
    format!("{r:.2}")
}

/// The monitors as the Displays page offers them.
pub fn monitors(outputs: &[Output]) -> Vec<Monitor> {
    outputs
        .iter()
        .map(|o| Monitor {
            name: o.name.clone(),
            label: o.label.clone(),
            modes: o
                .modes
                .iter()
                .map(|m| {
                    let rates = m.rates.iter().map(|r| rate(*r)).collect();
                    (format!("{}x{}", m.width, m.height), rates)
                })
                .collect(),
            preferred: o
                .modes
                .iter()
                .find(|m| m.preferred)
                .map(|m| format!("{}x{}", m.width, m.height)),
        })
        .collect()
}

/// Layout switch options with their keys as a person reads them.
pub fn switches(lang: Language) -> Vec<(String, String)> {
    let space = t(lang, "settings.space");
    keyboards::SWITCHES
        .iter()
        .map(|&option| {
            let label = match option {
                "grp:alt_shift_toggle" => "Alt+Shift".to_string(),
                "grp:ctrl_shift_toggle" => "Ctrl+Shift".to_string(),
                "grp:win_space_toggle" => format!("Super+{space}"),
                "grp:caps_toggle" => "Caps Lock".to_string(),
                "grp:menu_toggle" => "Menu".to_string(),
                "grp:lwin_toggle" => "Super (L)".to_string(),
                "grp:rwin_toggle" => "Super (R)".to_string(),
                other => other.to_string(),
            };
            (option.to_string(), label)
        })
        .collect()
}

fn rotation_name(r: Rotation) -> &'static str {
    match r {
        Rotation::Normal => "normal",
        Rotation::Left => "left",
        Rotation::Right => "right",
        Rotation::Inverted => "inverted",
    }
}

/// The size an output takes on the screen (turned on its side: swapped).
fn extent(o: &Output) -> (i32, i32) {
    let (w, h) = (o.width as i32, o.height as i32);
    match o.rotation {
        Rotation::Left | Rotation::Right => (h, w),
        _ => (w, h),
    }
}

/// Where `o` stands next to `other`, as the Position popup names it.
fn side_of(o: &Output, other: &Output) -> &'static str {
    let (w, h) = extent(other);
    if (o.x, o.y) == (other.x, other.y) {
        "mirror"
    } else if o.x >= other.x + w {
        "right"
    } else if o.x + extent(o).0 <= other.x {
        "left"
    } else if o.y + extent(o).1 <= other.y {
        "above"
    } else if o.y >= other.y + h {
        "below"
    } else {
        "right"
    }
}

/// The current state as the pages' values.
pub fn values(outputs: &[Output], keyboard: &Keyboard) -> HashMap<String, String> {
    let mut v = HashMap::new();
    for (i, o) in outputs.iter().enumerate() {
        let key = |what: &str| format!("display.{}.{what}", o.name);
        v.insert(key("active"), o.active.to_string());
        v.insert(key("primary"), o.primary.to_string());
        v.insert(key("mode"), format!("{}x{}", o.width, o.height));
        v.insert(key("rate"), rate(o.rate));
        v.insert(key("rotation"), rotation_name(o.rotation).to_string());
        // Placed next to the first monitor that is on.
        if i > 0
            && let Some(first) = outputs.iter().find(|f| f.active && f.name != o.name)
        {
            v.insert(key("side"), format!("{}:{}", side_of(o, first), first.name));
        }
    }
    let layouts: Vec<&str> = keyboard.layouts.iter().map(|(l, _)| l.as_str()).collect();
    v.insert("keyboard.layouts".into(), layouts.join(","));
    v.insert(
        "keyboard.switch".into(),
        keyboard.switch.clone().unwrap_or_default(),
    );
    v.insert("keyboard.delay".into(), keyboard.delay.to_string());
    v.insert("keyboard.rate".into(), keyboard.rate.to_string());
    v.insert(
        "keyboard.numlock".into(),
        keyboard.numlock.unwrap_or(false).to_string(),
    );
    v
}

/// The outputs as they should be after the row `key` got `value`; `None`
/// when it changes nothing.
pub fn changed_outputs(outputs: &[Output], key: &str, value: &str) -> Option<Vec<Output>> {
    let rest = key.strip_prefix("display.")?;
    let (name, what) = rest.rsplit_once('.')?;
    let mut new = outputs.to_vec();
    let i = new.iter().position(|o| o.name == name)?;
    match what {
        "mode" => {
            let (w, h) = value.split_once('x')?;
            let (w, h) = (w.parse().ok()?, h.parse().ok()?);
            let mode = new[i]
                .modes
                .iter()
                .find(|m| (m.width, m.height) == (w, h))?;
            // Keep the rate if the new resolution has it, else its best.
            let keep = mode.rates.iter().find(|r| rate(**r) == rate(new[i].rate));
            let r = keep.or(mode.rates.first()).copied()?;
            (new[i].width, new[i].height, new[i].rate) = (w, h, r);
        }
        "rate" => {
            let mode = new[i]
                .modes
                .iter()
                .find(|m| (m.width, m.height) == (new[i].width, new[i].height))?;
            new[i].rate = mode.rates.iter().find(|r| rate(**r) == value).copied()?;
        }
        "rotation" => {
            new[i].rotation = match value {
                "left" => Rotation::Left,
                "right" => Rotation::Right,
                "inverted" => Rotation::Inverted,
                _ => Rotation::Normal,
            };
        }
        "active" => {
            let on = value == "true";
            if on && !new[i].active {
                // Back on: its best mode, to the right of the first one on.
                let mode = new[i]
                    .modes
                    .iter()
                    .find(|m| m.preferred)
                    .or(new[i].modes.first())?
                    .clone();
                (new[i].width, new[i].height) = (mode.width, mode.height);
                new[i].rate = mode.rates.first().copied()?;
                new[i].active = true;
                if let Some(first) = new.iter().find(|o| o.active && o.name != name) {
                    let first = first.name.clone();
                    displays::place(&mut new, name, &first, Side::Right);
                }
            } else if !on {
                new[i].active = false;
                new[i].primary = false;
            }
        }
        "primary" => {
            let main = value == "true";
            for (n, o) in new.iter_mut().enumerate() {
                o.primary = main && n == i;
            }
        }
        "side" => {
            let (side, other) = value.split_once(':')?;
            let side = match side {
                "left" => Side::Left,
                "above" => Side::Above,
                "below" => Side::Below,
                "mirror" => Side::Mirror,
                _ => Side::Right,
            };
            displays::place(&mut new, name, other, side);
        }
        _ => return None,
    }
    (new != outputs).then_some(new)
}

/// A Keyboard page row changed: apply it (Xfce keeps it for the next login).
pub fn change_keyboard(key: &str, value: &str) -> Result<(), String> {
    let mut k = keyboards::current();
    match key {
        "keyboard.add" => {
            if !k.layouts.iter().any(|(l, _)| l == value) {
                k.layouts.push((value.to_string(), String::new()));
            }
            keyboards::set_layouts(&k)
        }
        "keyboard.switch" => {
            k.switch = (!value.is_empty()).then(|| value.to_string());
            keyboards::set_layouts(&k)
        }
        "keyboard.delay" => keyboards::set_repeat(value.parse().map_err(|_| value)?, k.rate),
        "keyboard.rate" => keyboards::set_repeat(k.delay, value.parse().map_err(|_| value)?),
        "keyboard.numlock" => keyboards::set_numlock(value == "true"),
        other => match other.strip_prefix("keyboard.remove.") {
            Some(i) => {
                let i: usize = i.parse().map_err(|_| other.to_string())?;
                if i < k.layouts.len() && k.layouts.len() > 1 {
                    k.layouts.remove(i);
                }
                keyboards::set_layouts(&k)
            }
            None => Err(format!("нет такой настройки: {other}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = "\
Screen 0: minimum 320 x 200, current 4480 x 1440, maximum 16384 x 16384
eDP-1 connected primary 1920x1200+0+0 (normal left inverted right x axis y axis) 345mm x 215mm
   2560x1600    165.00 +  60.00
   1920x1200    165.00*   59.95
HDMI-1 connected 2560x1440+1920+0 (normal left inverted right x axis y axis) 600mm x 340mm
   2560x1440     59.95*+
   1920x1080     60.00    50.00
";

    fn two() -> Vec<Output> {
        displays::parse(TWO)
    }

    fn keyboard() -> Keyboard {
        Keyboard {
            layouts: vec![("us".into(), String::new()), ("ru".into(), String::new())],
            switch: Some("grp:alt_shift_toggle".into()),
            other_options: vec![],
            delay: 500,
            rate: 20,
            numlock: None,
        }
    }

    #[test]
    fn values_name_the_current_state() {
        let v = values(&two(), &keyboard());
        assert_eq!(v["display.eDP-1.mode"], "1920x1200");
        assert_eq!(v["display.eDP-1.rate"], "165.00");
        assert_eq!(v["display.eDP-1.primary"], "true");
        assert_eq!(v["display.HDMI-1.side"], "right:eDP-1");
        assert_eq!(v["keyboard.layouts"], "us,ru");
        assert_eq!(v["keyboard.numlock"], "false");
        // The page's rates are spelled the same way as the values.
        let m = monitors(&two());
        assert_eq!(
            m[0].modes[1],
            ("1920x1200".into(), vec!["165.00".into(), "59.95".into()])
        );
        assert_eq!(m[0].preferred.as_deref(), Some("2560x1600"));
    }

    #[test]
    fn a_new_resolution_keeps_the_rate_it_can() {
        let new = changed_outputs(&two(), "display.eDP-1.mode", "2560x1600").expect("changed");
        assert_eq!(
            (new[0].width, new[0].height, rate(new[0].rate)),
            (2560, 1600, "165.00".into())
        );
        let new = changed_outputs(&two(), "display.HDMI-1.mode", "1920x1080").expect("changed");
        assert_eq!(
            rate(new[1].rate),
            "60.00",
            "59.95 is not there: the first one"
        );
        let new = changed_outputs(&two(), "display.eDP-1.rate", "59.95").expect("changed");
        assert_eq!(rate(new[0].rate), "59.95");
        assert!(changed_outputs(&two(), "display.eDP-1.rate", "30.00").is_none());
        assert!(changed_outputs(&two(), "display.eDP-1.mode", "1920x1200").is_none());
    }

    #[test]
    fn main_display_off_and_on_and_position() {
        let new = changed_outputs(&two(), "display.HDMI-1.primary", "true").expect("changed");
        assert!(!new[0].primary && new[1].primary, "only one main display");
        let off = changed_outputs(&two(), "display.HDMI-1.active", "false").expect("changed");
        assert!(!off[1].active);
        let on = changed_outputs(&off, "display.HDMI-1.active", "true").expect("changed");
        assert!(on[1].active);
        assert_eq!(
            (on[1].width, on[1].x),
            (2560, 1920),
            "its best mode, on the right"
        );
        let left = changed_outputs(&two(), "display.HDMI-1.side", "left:eDP-1").expect("changed");
        assert_eq!(
            values(&left, &keyboard())["display.HDMI-1.side"],
            "left:eDP-1"
        );
        assert!(changed_outputs(&two(), "display.NONE.mode", "1x1").is_none());
    }
}
