//! RandR state and Xfce's persistent Default display profile.

use crate::sys;

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub rates: Vec<f64>,
    pub preferred: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Normal,
    Left,
    Right,
    Inverted,
}

impl Rotation {
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Left => "left",
            Self::Right => "right",
            Self::Inverted => "inverted",
        }
    }

    fn degrees(self) -> i32 {
        match self {
            Self::Normal => 0,
            Self::Left => 90,
            Self::Right => 270,
            Self::Inverted => 180,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
    Mirror,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub name: String,
    pub label: String,
    pub builtin: bool,
    pub active: bool,
    pub primary: bool,
    pub x: i32,
    pub y: i32,
    /// Unrotated mode dimensions; placement uses the rotated dimensions.
    pub width: u32,
    pub height: u32,
    pub rate: f64,
    pub rotation: Rotation,
    pub modes: Vec<Mode>,
}

fn dimensions(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn geometry(text: &str) -> Option<(u32, u32, i32, i32)> {
    let start = text.find(['+', '-'])?;
    let (w, h) = dimensions(&text[..start])?;
    let coords = &text[start..];
    let split = coords[1..].find(['+', '-'])? + 1;
    Some((
        w,
        h,
        coords[..split].parse().ok()?,
        coords[split..].parse().ok()?,
    ))
}

pub fn parse(xrandr_query: &str) -> Vec<Output> {
    let mut outputs: Vec<Output> = Vec::new();
    let mut connected = false;
    for line in xrandr_query.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        if !line.starts_with(char::is_whitespace) {
            connected = words.get(1) == Some(&"connected");
            if !connected {
                continue;
            }
            let name = words[0];
            let geo = words.iter().find_map(|s| geometry(s));
            let rotation = words
                .iter()
                .take_while(|s| !s.starts_with('('))
                .find_map(|s| match *s {
                    "left" => Some(Rotation::Left),
                    "right" => Some(Rotation::Right),
                    "inverted" => Some(Rotation::Inverted),
                    _ => None,
                })
                .unwrap_or(Rotation::Normal);
            let (mut width, mut height, x, y) = geo.unwrap_or_default();
            if matches!(rotation, Rotation::Left | Rotation::Right) {
                std::mem::swap(&mut width, &mut height);
            }
            outputs.push(Output {
                name: name.into(),
                label: name.into(),
                builtin: ["eDP", "LVDS", "DSI"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix)),
                active: geo.is_some(),
                primary: words.contains(&"primary"),
                x,
                y,
                width,
                height,
                rate: 0.0,
                rotation,
                modes: Vec::new(),
            });
        } else if connected && let Some(output) = outputs.last_mut() {
            let Some((width, height)) = words.first().and_then(|s| dimensions(s)) else {
                continue;
            };
            let mut mode = Mode {
                width,
                height,
                rates: Vec::new(),
                preferred: false,
            };
            for word in &words[1..] {
                mode.preferred |= word.contains('+');
                if let Ok(rate) = word.trim_end_matches(['*', '+']).parse::<f64>()
                    && rate.is_finite()
                    && rate > 0.0
                {
                    mode.rates.push(rate);
                    if word.contains('*') {
                        output.width = width;
                        output.height = height;
                        output.rate = rate;
                    }
                }
            }
            if !mode.rates.is_empty() {
                output.modes.push(mode);
            }
        }
    }
    outputs
}

pub fn outputs() -> Vec<Output> {
    let mut outputs = parse(&sys::output(&["xrandr", "--query"]).unwrap_or_default());
    for output in &mut outputs {
        if let Some(label) = sys::xget("displays", &format!("/Default/{}", output.name))
            && !label.is_empty()
        {
            output.label = label;
        }
    }
    outputs
}

fn size(output: &Output) -> (i32, i32) {
    let (w, h) = if matches!(output.rotation, Rotation::Left | Rotation::Right) {
        (output.height, output.width)
    } else {
        (output.width, output.height)
    };
    (w.min(i32::MAX as u32) as i32, h.min(i32::MAX as u32) as i32)
}

/// Missing names, self-placement and mirroring without a common mode are no-ops.
/// Negative positions are translated for all active outputs to RandR's origin.
pub fn place(outputs: &mut [Output], moved: &str, next_to: &str, side: Side) {
    let Some(i) = outputs.iter().position(|o| o.name == moved) else {
        return;
    };
    let Some(j) = outputs.iter().position(|o| o.name == next_to) else {
        return;
    };
    if i == j {
        return;
    }
    if side == Side::Mirror {
        let common = outputs[j].modes.iter().find_map(|a| {
            outputs[i].modes.iter().find_map(|b| {
                if (a.width, a.height) != (b.width, b.height) {
                    return None;
                }
                // Separate CRTCs can mirror one resolution at different rates.
                let shared = a.rates.iter().find(|r| b.rates.contains(r)).copied();
                Some((
                    a.width,
                    a.height,
                    shared.or_else(|| a.rates.first().copied())?,
                    shared.or_else(|| b.rates.first().copied())?,
                ))
            })
        });
        let Some((width, height, anchor_rate, moved_rate)) = common else {
            return;
        };
        for n in [i, j] {
            outputs[n].width = width;
            outputs[n].height = height;
            outputs[n].rate = if n == j { anchor_rate } else { moved_rate };
            outputs[n].rotation = Rotation::Normal;
            outputs[n].active = true;
        }
    }
    let (w, h) = size(&outputs[i]);
    let (aw, ah) = size(&outputs[j]);
    let (x, y) = (outputs[j].x, outputs[j].y);
    (outputs[i].x, outputs[i].y) = match side {
        Side::Left => (x.saturating_sub(w), y),
        Side::Right => (x.saturating_add(aw), y),
        Side::Above => (x, y.saturating_sub(h)),
        Side::Below => (x, y.saturating_add(ah)),
        Side::Mirror => (x, y),
    };
    outputs[i].active = true;
    normalize(outputs);
}

fn normalize(outputs: &mut [Output]) {
    let x = outputs
        .iter()
        .filter(|o| o.active)
        .map(|o| o.x)
        .min()
        .unwrap_or(0)
        .min(0);
    let y = outputs
        .iter()
        .filter(|o| o.active)
        .map(|o| o.y)
        .min()
        .unwrap_or(0)
        .min(0);
    for output in outputs.iter_mut().filter(|o| o.active) {
        output.x = output.x.saturating_sub(x);
        output.y = output.y.saturating_sub(y);
    }
}

pub fn xrandr_args(outputs: &[Output]) -> Vec<String> {
    let mut outputs = outputs.to_vec();
    normalize(&mut outputs);
    let mut args = Vec::new();
    for o in &outputs {
        args.extend(["--output".into(), o.name.clone()]);
        if !o.active {
            args.push("--off".into());
            continue;
        }
        args.extend([
            "--mode".into(),
            format!("{}x{}", o.width, o.height),
            "--rate".into(),
            o.rate.to_string(),
            "--pos".into(),
            format!("{}x{}", o.x, o.y),
            "--rotate".into(),
            o.rotation.name().into(),
        ]);
        if o.primary {
            args.push("--primary".into());
        }
    }
    args
}

pub fn apply(outputs: &[Output]) -> Result<(), String> {
    if !outputs.iter().any(|o| o.active) {
        return Err("Нельзя выключить все экраны".into());
    }
    let result = sys::command("xrandr")
        .args(xrandr_args(outputs))
        .output()
        .map_err(|e| format!("Не удалось запустить xrandr: {e}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Не удалось настроить экраны: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

// Xfce 4.20, commit d56c7b38b48cf639f2aecd4acc7bcada24c1f478:
// common/xfce-randr.c:403-498 (called by dialogs/display-settings/main.c).
// Label: string; Active/Primary: bool; Resolution/Reflection: string;
// RefreshRate: double; Rotation (degrees)/Position/X/Y: int.
// dialogs/display-settings/main.c:1308: ActiveProfile is a string.
pub fn xfconf_props(outputs: &[Output]) -> Vec<(String, &'static str, String)> {
    let mut outputs = outputs.to_vec();
    normalize(&mut outputs);
    let mut props = Vec::new();
    for o in &outputs {
        let base = format!("/Default/{}", o.name);
        props.push((base.clone(), "string", o.label.clone()));
        props.push((format!("{base}/Active"), "bool", o.active.to_string()));
        if !o.active {
            continue;
        }
        for (suffix, kind, value) in [
            ("Resolution", "string", format!("{}x{}", o.width, o.height)),
            ("RefreshRate", "double", o.rate.to_string()),
            ("Rotation", "int", o.rotation.degrees().to_string()),
            ("Reflection", "string", "0".into()),
            ("Primary", "bool", o.primary.to_string()),
            ("Position/X", "int", o.x.to_string()),
            ("Position/Y", "int", o.y.to_string()),
        ] {
            props.push((format!("{base}/{suffix}"), kind, value));
        }
    }
    props.push(("/ActiveProfile".into(), "string", "Default".into()));
    props
}

pub fn save(outputs: &[Output]) -> Result<(), String> {
    // Never reset the output subtree: EDID, Scale and other unmodelled keys survive.
    for (property, kind, value) in xfconf_props(outputs) {
        if !sys::xset("displays", &property, kind, &value) {
            return Err(format!("Не удалось сохранить настройку экрана: {property}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUERY: &str = "Screen 0: minimum 320 x 200, current 1920 x 1200, maximum 16384 x 16384
eDP-1 connected 1920x1200+0+0 (normal left inverted right x axis y axis) 345mm x 215mm
   2560x1600    165.00 +  60.00 +  59.99    59.97
   2560x1440     59.99    59.99    59.96    59.95
   1920x1200    165.00*   59.88    59.95
   1920x1080    165.00    60.01    59.97    59.96    59.93
   1024x768i     86.96
   1024x576      59.95    59.96    59.90    59.82
HDMI-1 disconnected (normal left inverted right x axis y axis)
";
    const TWO: &str = "Screen 0: minimum 8 x 8, current 1920 x 1080, maximum 32767 x 32767
eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 309mm x 174mm
   1920x1080     60.00*+ 59.93
   1280x720      60.00
HDMI-1 connected (normal left inverted right x axis y axis)
   2560x1440     59.95 +
   1920x1080     60.00 50.00
   1280x720      60.00
DP-1 disconnected (normal left inverted right x axis y axis)
   800x600       60.00
";

    #[test]
    fn laptop_query() {
        let outputs = parse(QUERY);
        assert_eq!(outputs.len(), 1);
        let o = &outputs[0];
        assert_eq!((&*o.name, &*o.label), ("eDP-1", "eDP-1"));
        assert!(o.builtin && o.active && !o.primary);
        assert_eq!(
            (o.width, o.height, o.rate, o.x, o.y),
            (1920, 1200, 165.0, 0, 0)
        );
        assert_eq!(o.rotation, Rotation::Normal);
        assert_eq!(o.modes.len(), 5);
        assert!(o.modes[0].preferred);
        assert_eq!(o.modes[0].rates, [165.0, 60.0, 59.99, 59.97]);
        assert!(!o.modes[2].preferred);
    }

    #[test]
    fn connected_inactive_and_disconnected_modes() {
        let outputs = parse(TWO);
        assert_eq!(outputs.len(), 2);
        assert!(outputs[0].primary);
        let hdmi = &outputs[1];
        assert!(!hdmi.builtin && !hdmi.active);
        assert_eq!((hdmi.width, hdmi.height), (0, 0));
        assert_eq!(hdmi.modes.len(), 3);
        assert_eq!(hdmi.modes[0].width, 2560);
        assert!(hdmi.modes[0].preferred);
    }

    #[test]
    fn rotated_signed_geometry_and_combined_flags() {
        let o = parse("DP-2 connected primary 1080x1920-1080+24 left (normal left inverted right x axis y axis)\n   1920x1080 59.94*+\n").remove(0);
        assert_eq!((o.width, o.height, o.x, o.y), (1920, 1080, -1080, 24));
        assert_eq!(o.rotation, Rotation::Left);
        assert_eq!(size(&o), (1080, 1920));
        assert!(o.modes[0].preferred);
        assert_eq!(o.rate, 59.94);
    }

    #[test]
    fn malformed_and_empty_queries() {
        assert!(parse("").is_empty());
        assert!(
            parse("Screen 0: minimum 320 x 200, current 1920 x 1200, maximum 16384 x 16384\n")
                .is_empty()
        );
        let o =
            parse("DSI-1 connected\n   broken 60.00\n   800x600 NaN inf 0\n   1024x768i 86.96\n");
        assert!(o[0].builtin);
        assert!(o[0].modes.is_empty());
    }

    #[test]
    fn place_all_edges_with_rotation() {
        for (side, expected_moved, expected_anchor) in [
            (Side::Right, (1920, 0), (0, 0)),
            (Side::Below, (0, 1080), (0, 0)),
            (Side::Left, (0, 0), (1440, 0)),
            (Side::Above, (0, 0), (0, 2560)),
        ] {
            let mut o = parse(TWO);
            o[1].width = 2560;
            o[1].height = 1440;
            o[1].rotation = Rotation::Right;
            place(&mut o, "HDMI-1", "eDP-1", side);
            assert_eq!((o[1].x, o[1].y), expected_moved);
            assert_eq!((o[0].x, o[0].y), expected_anchor);
            assert!(o[1].active);
        }
    }

    #[test]
    fn mirror_chooses_common_mode_and_rate() {
        let mut o = parse(TWO);
        o[0].rotation = Rotation::Left;
        place(&mut o, "HDMI-1", "eDP-1", Side::Mirror);
        for output in &o {
            assert!(output.active);
            assert_eq!(
                (output.width, output.height, output.rate),
                (1920, 1080, 60.0)
            );
            assert_eq!((output.x, output.y), (0, 0));
            assert_eq!(output.rotation, Rotation::Normal);
        }
    }

    #[test]
    fn invalid_placement_is_unchanged() {
        let mut o = parse(TWO);
        let before = o.clone();
        place(&mut o, "absent", "eDP-1", Side::Left);
        place(&mut o, "eDP-1", "absent", Side::Right);
        place(&mut o, "eDP-1", "eDP-1", Side::Mirror);
        assert_eq!(o, before);
        o[1].modes.clear();
        let before = o.clone();
        place(&mut o, "HDMI-1", "eDP-1", Side::Mirror);
        assert_eq!(o, before);
    }

    #[test]
    fn mirror_allows_different_refresh_rates_for_the_same_resolution() {
        let mut o = parse(TWO);
        for mode in &mut o[1].modes {
            mode.rates = vec![75.0];
        }
        place(&mut o, "HDMI-1", "eDP-1", Side::Mirror);
        assert_eq!((o[0].width, o[0].height), (o[1].width, o[1].height));
        assert_eq!((o[0].rate, o[1].rate), (60.0, 75.0));
        assert_eq!((o[0].x, o[0].y), (o[1].x, o[1].y));
    }

    #[test]
    fn one_command_includes_disabled_output() {
        let o = parse(TWO);
        assert_eq!(
            xrandr_args(&o),
            [
                "--output",
                "eDP-1",
                "--mode",
                "1920x1080",
                "--rate",
                "60",
                "--pos",
                "0x0",
                "--rotate",
                "normal",
                "--primary",
                "--output",
                "HDMI-1",
                "--off",
            ]
        );
    }

    #[test]
    fn arguments_and_saved_positions_share_origin() {
        let mut o = parse(TWO);
        o[0].x = -1920;
        o[0].y = -20;
        o[0].rotation = Rotation::Inverted;
        assert!(xrandr_args(&o).windows(2).any(|a| a == ["--pos", "0x0"]));
        let props = xfconf_props(&o);
        assert!(props.contains(&("/Default/eDP-1/Position/X".into(), "int", "0".into())));
        assert!(props.contains(&("/Default/eDP-1/Rotation".into(), "int", "180".into())));
        assert_eq!((o[0].x, o[0].y), (-1920, -20));
    }

    #[test]
    fn saved_profile_exact_types_and_inactive_keys() {
        let mut o = parse(TWO);
        o[0].label = "Ноутбук".into();
        let expected = [
            ("/Default/eDP-1", "string", "Ноутбук"),
            ("/Default/eDP-1/Active", "bool", "true"),
            ("/Default/eDP-1/Resolution", "string", "1920x1080"),
            ("/Default/eDP-1/RefreshRate", "double", "60"),
            ("/Default/eDP-1/Rotation", "int", "0"),
            ("/Default/eDP-1/Reflection", "string", "0"),
            ("/Default/eDP-1/Primary", "bool", "true"),
            ("/Default/eDP-1/Position/X", "int", "0"),
            ("/Default/eDP-1/Position/Y", "int", "0"),
            ("/Default/HDMI-1", "string", "HDMI-1"),
            ("/Default/HDMI-1/Active", "bool", "false"),
            ("/ActiveProfile", "string", "Default"),
        ]
        .map(|(p, t, v)| (p.into(), t, v.into()));
        assert_eq!(xfconf_props(&o), expected);
    }

    #[test]
    fn all_rotation_degrees_match_randr() {
        let mut o = parse(QUERY);
        for (rotation, degrees, name) in [
            (Rotation::Normal, "0", "normal"),
            (Rotation::Left, "90", "left"),
            (Rotation::Right, "270", "right"),
            (Rotation::Inverted, "180", "inverted"),
        ] {
            o[0].rotation = rotation;
            assert!(xfconf_props(&o).contains(&(
                "/Default/eDP-1/Rotation".into(),
                "int",
                degrees.into()
            )));
            assert!(xrandr_args(&o).windows(2).any(|a| a == ["--rotate", name]));
        }
    }
}
