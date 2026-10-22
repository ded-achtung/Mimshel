//! Состояние системы для строки меню — звук и батарея — и разбор ответов
//! программ, которые его сообщают (`pactl`, `upower`; их запускает служба,
//! с `LC_ALL=C`, чтобы ответ был на одном языке).

/// Устройство вывода звука.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sink {
    /// Внутреннее имя (для `pactl set-default-sink`).
    pub name: String,
    /// Название для человека.
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sound {
    /// Громкость устройства по умолчанию, проценты.
    pub volume: u32,
    pub muted: bool,
    pub sinks: Vec<Sink>,
    /// Имя устройства по умолчанию.
    pub default: String,
}

impl Sound {
    /// Иконка громкости в строке меню.
    pub fn icon(&self) -> &'static str {
        match self.volume {
            _ if self.muted => "audio-volume-muted-symbolic",
            0 => "audio-volume-muted-symbolic",
            1..=33 => "audio-volume-low-symbolic",
            34..=66 => "audio-volume-medium-symbolic",
            _ => "audio-volume-high-symbolic",
        }
    }
}

/// Громкость из ответа `pactl get-sink-volume`: первое число с «%».
pub fn parse_volume(text: &str) -> Option<u32> {
    text.split_whitespace()
        .find_map(|w| w.strip_suffix('%')?.parse().ok())
}

/// Выключен ли звук — ответ `pactl get-sink-mute` («Mute: yes»).
pub fn parse_mute(text: &str) -> bool {
    text.trim().ends_with("yes")
}

/// Устройства из ответа `pactl list sinks` (строки `Name:` и `Description:`).
pub fn parse_sinks(text: &str) -> Vec<Sink> {
    let mut out: Vec<Sink> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("Name:") {
            out.push(Sink {
                name: name.trim().to_string(),
                description: name.trim().to_string(),
            });
        } else if let Some(d) = line.strip_prefix("Description:")
            && let Some(last) = out.last_mut()
        {
            last.description = d.trim().to_string();
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    pub percent: u32,
    /// Как в UPower: `charging`, `discharging`, `fully-charged`, `pending-charge`…
    pub state: String,
    /// Иконка от UPower (уже с нужным уровнем заряда).
    pub icon: String,
    /// Сколько осталось («2,5 hours») — до разряда или до полного заряда.
    pub time: Option<String>,
}

impl Battery {
    /// Состояние словами.
    pub fn state_text(&self) -> String {
        let time = self.time.as_deref().map(human_time);
        match (self.state.as_str(), time) {
            ("fully-charged", _) => "Полностью заряжен".into(),
            ("charging", Some(t)) => format!("Заряжается, до полного {t}"),
            ("charging", None) => "Заряжается".into(),
            ("discharging", Some(t)) => format!("От аккумулятора, осталось {t}"),
            ("discharging", None) => "От аккумулятора".into(),
            ("pending-charge", _) => "Ожидает зарядки".into(),
            ("empty", _) => "Разряжен".into(),
            _ => "Состояние неизвестно".into(),
        }
    }
}

/// «2,5 hours» / «40,0 minutes» → «2,5 ч» / «40 мин».
fn human_time(t: &str) -> String {
    let mut it = t.split_whitespace();
    let (Some(n), Some(unit)) = (it.next(), it.next()) else {
        return t.to_string();
    };
    match unit {
        u if u.starts_with("hour") => format!("{n} ч"),
        u if u.starts_with("minute") => format!("{} мин", n.split([',', '.']).next().unwrap_or(n)),
        u if u.starts_with("second") => "меньше минуты".into(),
        _ => t.to_string(),
    }
}

/// Батарея из ответа `upower -i …/DisplayDevice`. Нет батареи — `None`.
pub fn parse_upower(text: &str) -> Option<Battery> {
    let field = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().trim_matches('\'').to_string())
        })
    };
    if field("present").as_deref() != Some("yes") {
        return None;
    }
    let percent = field("percentage")?
        .trim_end_matches('%')
        .split([',', '.'])
        .next()?
        .parse()
        .ok()?;
    Some(Battery {
        percent,
        state: field("state").unwrap_or_default(),
        icon: field("icon-name").unwrap_or_else(|| "battery-symbolic".into()),
        time: field("time to empty").or_else(|| field("time to full")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound() {
        let v = "Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: 26214 /  40% / -23.88 dB\n        balance 0.00\n";
        assert_eq!(parse_volume(v), Some(40));
        assert!(parse_mute("Mute: yes\n"));
        assert!(!parse_mute("Mute: no\n"));
        let list = "Sink #56\n\tState: SUSPENDED\n\tName: alsa_output.analog\n\tDescription: Встроенное аудио\n\
                    Sink #57\n\tName: hdmi\n\tDescription: HDMI\n";
        let sinks = parse_sinks(list);
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].name, "alsa_output.analog");
        assert_eq!(sinks[0].description, "Встроенное аудио");
        let mut s = Sound {
            volume: 40,
            muted: false,
            sinks,
            default: "hdmi".into(),
        };
        assert_eq!(s.icon(), "audio-volume-medium-symbolic");
        s.muted = true;
        assert_eq!(s.icon(), "audio-volume-muted-symbolic");
    }

    #[test]
    fn battery() {
        let full = "  native-path:          BAT0\n  power supply:         yes\n    present:             yes\n    state:               fully-charged\n    percentage:          100%\n    icon-name:          'battery-full-charged-symbolic'\n";
        let b = parse_upower(full).unwrap();
        assert_eq!(b.percent, 100);
        assert_eq!(b.icon, "battery-full-charged-symbolic");
        assert_eq!(b.state_text(), "Полностью заряжен");

        let low = "    present:             yes\n    state:               discharging\n    time to empty:       2,5 hours\n    percentage:          41,5%\n    icon-name:          'battery-level-40-symbolic'\n";
        let b = parse_upower(low).unwrap();
        assert_eq!(b.percent, 41);
        assert_eq!(b.state_text(), "От аккумулятора, осталось 2,5 ч");

        assert_eq!(parse_upower("    present:             no\n"), None);
        assert_eq!(parse_upower(""), None);
    }
}
