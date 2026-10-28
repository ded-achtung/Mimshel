//! Состояние системы для строки меню — звук и батарея — и разбор того, что
//! сообщают их источники: вывод `pactl` (служба запускает его с `LC_ALL=C`,
//! чтобы ответ был на одном языке) и файлы ядра `/sys/class/power_supply`.

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
    /// Программы, которые сейчас выводят звук, — у каждой своя громкость.
    pub apps: Vec<AppSound>,
}

/// Программа, которая выводит звук: все её потоки вместе (у Firefox — по
/// потоку на вкладку), как в микшере громкости Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSound {
    pub name: String,
    /// Номера потоков — для `pactl set-sink-input-volume`.
    pub inputs: Vec<u32>,
    /// Громкость первого потока, проценты.
    pub volume: u32,
    /// Все потоки без звука.
    pub muted: bool,
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

/// Громкость и mute устройства по умолчанию из одного ответа `pactl list
/// sinks` — всё нужное уже есть в этом выводе, отдельные запуски pactl на
/// каждый вопрос не нужны. Если устройство `default` в выводе не нашлось
/// (или имя не известно), берётся первое.
/// Программы со звуком из ответа `pactl list sink-inputs`: потоки одной
/// программы (по `application.name`, иначе по имени файла программы) — в одну
/// строку, в порядке появления.
pub fn parse_app_sounds(text: &str) -> Vec<AppSound> {
    parse_app_sounds_lang(text, mimshel_core::i18n::Language::Ru)
}

/// Language-aware variant of `parse_app_sounds`.
pub fn parse_app_sounds_lang(text: &str, lang: mimshel_core::i18n::Language) -> Vec<AppSound> {
    let mut out: Vec<AppSound> = Vec::new();
    for block in text.split("Sink Input #").skip(1) {
        let Some(index) = block
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let lines = || block.lines().map(str::trim);
        let prop = |key: &str| {
            lines().find_map(|l| {
                let v = l.strip_prefix(key)?.trim_start().strip_prefix('=')?;
                Some(v.trim().trim_matches('"').to_string())
            })
        };
        let name = prop("application.name")
            .or_else(|| prop("application.process.binary"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| mimshel_core::i18n::t(lang, "menubar.sound_unnamed").into());
        let volume = lines()
            .find(|l| l.starts_with("Volume:"))
            .and_then(parse_volume)
            .unwrap_or(100);
        let muted = lines()
            .find(|l| l.starts_with("Mute:"))
            .is_some_and(parse_mute);
        match out.iter_mut().find(|a| a.name == name) {
            Some(app) => {
                app.inputs.push(index);
                app.muted &= muted;
            }
            None => out.push(AppSound {
                name,
                inputs: vec![index],
                volume,
                muted,
            }),
        }
    }
    out
}

pub fn parse_default_state(text: &str, default: &str) -> Option<(u32, bool)> {
    let blocks: Vec<&str> = text.split("Sink #").skip(1).collect();
    let block = if default.is_empty() {
        blocks.first().copied()
    } else {
        blocks
            .iter()
            .find(|b| {
                b.lines().any(|l| {
                    l.trim()
                        .strip_prefix("Name:")
                        .is_some_and(|n| n.trim() == default)
                })
            })
            .copied()
            .or_else(|| blocks.first().copied())
    }?;
    let volume = block
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Volume:"))
        .and_then(parse_volume)?;
    let muted = block
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Mute:"))
        .is_some_and(parse_mute);
    Some((volume, muted))
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
    /// Состояние словами (русский по умолчанию — старые вызовы и тесты).
    pub fn state_text(&self) -> String {
        self.state_text_lang(mimshel_core::i18n::Language::Ru)
    }

    /// Состояние словами на языке `lang`.
    pub fn state_text_lang(&self, lang: mimshel_core::i18n::Language) -> String {
        let time = self.time.as_deref().map(|v| human_time_lang(v, lang));
        let tr = |k: &'static str| mimshel_core::i18n::t(lang, k);
        match (self.state.as_str(), time) {
            ("fully-charged", _) => tr("battery.fully_charged").into(),
            ("charging", Some(t)) => tr("battery.charging_full").replace("{t}", &t),
            ("charging", None) => tr("battery.charging").into(),
            ("discharging", Some(t)) => tr("battery.on_battery_left").replace("{t}", &t),
            ("discharging", None) => tr("battery.on_battery").into(),
            ("pending-charge", _) => tr("battery.plugged_not_charging").into(),
            ("empty", _) => tr("battery.empty").into(),
            _ => tr("battery.unknown").into(),
        }
    }
}

/// "2,5 hours" / "40,0 minutes" -> "2,5 ч" / "40 мин" in the given language.
fn human_time_lang(t: &str, lang: mimshel_core::i18n::Language) -> String {
    let mut it = t.split_whitespace();
    let (Some(n), Some(unit)) = (it.next(), it.next()) else {
        return t.to_string();
    };
    let tr = |k: &'static str| mimshel_core::i18n::t(lang, k);
    match unit {
        u if u.starts_with("hour") => tr("battery.time_hour").replace("{n}", n),
        u if u.starts_with("minute") => {
            tr("battery.time_min").replace("{n}", n.split([',', '.']).next().unwrap_or(n))
        }
        u if u.starts_with("second") => tr("battery.less_than_minute").into(),
        _ => t.to_string(),
    }
}

/// Показания одной батареи из `/sys/class/power_supply`. Заряд ядро отдаёт
/// либо энергией (мкВт·ч), либо зарядом (мкА·ч), мощность — соответственно
/// в мкВт или мкА; на проценты и время это не влияет, единицы сокращаются.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SysBattery {
    /// Как пишет ядро: `Charging`, `Discharging`, `Full`, `Not charging`,
    /// `Unknown`.
    pub status: String,
    /// Остаток заряда (`energy_now` или `charge_now`).
    pub now: Option<u64>,
    /// Полный заряд (`energy_full`, `charge_full` или `charge_full_design`).
    pub full: Option<u64>,
    /// Текущая мощность (`power_now` или `current_now`).
    pub power: Option<u64>,
    /// Процент от ядра — запасной вариант, когда заряда в единицах нет.
    pub capacity: Option<u64>,
}

/// Сводная батарея из показаний ядра: несколько батарей складываются (как
/// это делал DisplayDevice в UPower). `on_ac` — подключено ли питание (блок
/// питания или USB; `None` — ядро не сообщает). Нет ни одной батареи — `None`.
pub fn sys_battery(bats: &[SysBattery], on_ac: Option<bool>) -> Option<Battery> {
    let present: Vec<&SysBattery> = bats
        .iter()
        .filter(|b| b.now.is_some() || b.capacity.is_some())
        .collect();
    if present.is_empty() {
        return None;
    }
    let sum = |f: fn(&SysBattery) -> Option<u64>| present.iter().filter_map(|b| f(b)).sum::<u64>();
    let (now, full, power) = (sum(|b| b.now), sum(|b| b.full), sum(|b| b.power));
    let percent = (now * 100).checked_div(full).unwrap_or_else(|| {
        // Заряда в единицах нет — средний процент из тех, что дало ядро.
        let caps: Vec<u64> = present.iter().filter_map(|b| b.capacity).collect();
        (caps.iter().sum::<u64>() / (caps.len() as u64).max(1)).min(100)
    });
    let percent = u32::try_from(percent).unwrap_or(100);
    let any = |s: &str| present.iter().any(|b| b.status == s);
    let full_now = present.iter().all(|b| b.status == "Full");
    // Питание отключили — разряд, даже если аккумулятор ещё с полсекунды
    // пишет «Charging» (так в VirtualBox: блок питания меняется первым).
    let state = if on_ac == Some(false) && !full_now {
        "discharging"
    } else if any("Charging") {
        "charging"
    } else if full_now {
        "fully-charged"
    } else if on_ac == Some(true) {
        // Питание подключено, но аккумулятор не заряжается: держит заряд
        // (ноутбук ограничивает зарядку) или ещё не переключился — сразу
        // после подключения он пару секунд пишет «Discharging».
        if percent >= 100 {
            "fully-charged"
        } else {
            "pending-charge"
        }
    } else if any("Discharging") || on_ac == Some(false) {
        "discharging"
    } else if any("Not charging") {
        "pending-charge"
    } else {
        ""
    };
    Some(Battery {
        percent,
        state: state.to_string(),
        icon: battery_icon(percent, state),
        time: battery_time(state, now, full, power),
    })
}

/// Имя значка как у UPower: уровень десятками, с `-charging` при зарядке.
fn battery_icon(percent: u32, state: &str) -> String {
    let tier = percent / 10 * 10;
    match state {
        "fully-charged" => "battery-full-charged-symbolic".to_string(),
        "charging" if percent >= 100 => "battery-full-charging-symbolic".to_string(),
        "charging" => format!("battery-level-{tier}-charging-symbolic"),
        // Подключено к сети, но не заряжается — вилка вместо молнии.
        "pending-charge" => format!("battery-level-{tier}-plugged-in-symbolic"),
        _ if tier == 0 => "battery-empty-symbolic".to_string(),
        _ => format!("battery-level-{tier}-symbolic"),
    }
}

/// Время до полного разряда или заряда в том же виде, в котором его выдавал
/// UPower («2,5 hours») — [`human_time_lang`] превращает его в «2,5 ч».
fn battery_time(state: &str, now: u64, full: u64, power: u64) -> Option<String> {
    if power == 0 {
        return None;
    }
    let left = match state {
        "discharging" => Some(now),
        "charging" => full.checked_sub(now),
        _ => None,
    }?;
    if left == 0 {
        return None;
    }
    let secs = left * 3600 / power;
    Some(if secs >= 3600 {
        format!("{:.1} hours", secs as f64 / 3600.0).replace('.', ",")
    } else if secs >= 60 {
        format!("{} minutes", secs / 60)
    } else {
        format!("{secs} seconds")
    })
}

/// Screen backlight from `/sys/class/backlight/DEVICE`: raw values, as the
/// kernel gives them (`max_brightness` differs per driver: 255, 65535…).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlight {
    /// Device name for logind's `SetBrightness("backlight", device, value)`.
    pub device: String,
    pub value: u32,
    pub max: u32,
}

impl Backlight {
    /// Brightness in percent, rounded.
    pub fn percent(&self) -> u32 {
        if self.max == 0 {
            return 0;
        }
        ((u64::from(self.value) * 100 + u64::from(self.max) / 2) / u64::from(self.max)) as u32
    }

    /// Raw value for `percent`. Never 0: a dark screen cannot be brought
    /// back with the slider you can no longer see — at least 1 % (and 1).
    pub fn value_for(&self, percent: u32) -> u32 {
        let floor = (self.max / 100).max(1).min(self.max);
        let v = u64::from(self.max) * u64::from(percent.min(100)) / 100;
        (v as u32).max(floor)
    }
}

/// Which backlight device to use when there are several (hybrid graphics,
/// ACPI next to the GPU driver): like systemd and GNOME — `firmware` first,
/// then `platform`, then `raw`; same type — the first name in order.
pub fn pick_backlight<'a>(devices: &[(&'a str, &str)]) -> Option<&'a str> {
    let rank = |t: &str| match t {
        "firmware" => 0,
        "platform" => 1,
        "raw" => 2,
        _ => 3,
    };
    let mut sorted: Vec<_> = devices.iter().filter(|(_, t)| rank(t) < 3).collect();
    sorted.sort_by_key(|(name, t)| (rank(t), *name));
    sorted.first().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_sounds_grouped_by_program() {
        let text = "Sink Input #89\n\tCorked: no\n\tMute: no\n\
                    \tVolume: front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / 0.00 dB\n\
                    \tProperties:\n\t\tapplication.name = \"Firefox\"\n\
                    \t\tapplication.process.binary = \"firefox\"\n\
                    Sink Input #90\n\tMute: yes\n\tVolume: front-left: 32768 /  50% / -18.06 dB\n\
                    \tProperties:\n\t\tapplication.process.binary = \"mpv\"\n\
                    Sink Input #91\n\tMute: no\n\tVolume: front-left: 26214 /  40% / -23.88 dB\n\
                    \tProperties:\n\t\tapplication.name = \"Firefox\"\n";
        let apps = parse_app_sounds(text);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "Firefox");
        assert_eq!(apps[0].inputs, [89, 91]);
        assert_eq!((apps[0].volume, apps[0].muted), (100, false));
        assert_eq!(apps[1].name, "mpv");
        assert_eq!((apps[1].volume, apps[1].muted), (50, true));
        assert!(parse_app_sounds("").is_empty());
    }

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
            apps: vec![],
        };
        assert_eq!(s.icon(), "audio-volume-medium-symbolic");
        s.muted = true;
        assert_eq!(s.icon(), "audio-volume-muted-symbolic");
    }

    #[test]
    fn default_sink_state() {
        // Один вывод `pactl list sinks` даёт и устройства, и громкость с
        // mute устройства по умолчанию — отдельные запуски pactl не нужны.
        let list = "Sink #56\n\tState: RUNNING\n\tName: alsa_output.analog\n\tMute: no\n\
                    \tVolume: front-left: 45875 /  70% / -9.31 dB,\n\t        front-right: 45875 /  70% / -9.31 dB\n\
                    \tBase Volume: 65536 / 100% / 0.00 dB\n\
                    Sink #57\n\tName: hdmi\n\tMute: yes\n\tVolume: front-left: 65536 / 100% / 0.00 dB\n";
        assert_eq!(parse_default_state(list, "hdmi"), Some((100, true)));
        assert_eq!(
            parse_default_state(list, "alsa_output.analog"),
            Some((70, false))
        );
        // Устройство по умолчанию не найдено или не известно — первое.
        assert_eq!(parse_default_state(list, "нет-такого"), Some((70, false)));
        assert_eq!(parse_default_state(list, ""), Some((70, false)));
        assert_eq!(parse_default_state("", "x"), None);
    }

    #[test]
    fn battery() {
        let one = [SysBattery {
            status: "Discharging".into(),
            now: Some(2_050_000),
            full: Some(5_000_000),
            power: Some(800_000),
            capacity: Some(41),
        }];
        let b = sys_battery(&one, None).unwrap();
        assert_eq!(b.percent, 41);
        assert_eq!(b.state, "discharging");
        assert_eq!(b.icon, "battery-level-40-symbolic");
        // 2,05 М мкВт·ч / 0,8 М мкВт ≈ 2,56 ч → «2,6 hours» → «2,6 ч».
        assert_eq!(b.state_text(), "От аккумулятора, осталось 2,6 ч");

        let full = [SysBattery {
            status: "Full".into(),
            now: Some(100),
            full: Some(100),
            ..SysBattery::default()
        }];
        let b = sys_battery(&full, None).unwrap();
        assert_eq!(b.state, "fully-charged");
        assert_eq!(b.icon, "battery-full-charged-symbolic");
        assert_eq!(b.state_text(), "Полностью заряжен");

        let charging = [SysBattery {
            status: "Charging".into(),
            now: Some(40),
            full: Some(100),
            power: Some(60),
            capacity: None,
        }];
        let b = sys_battery(&charging, None).unwrap();
        assert_eq!(b.state, "charging");
        assert_eq!(b.time.as_deref(), Some("1,0 hours"));
        assert_eq!(b.icon, "battery-level-40-charging-symbolic");
        assert_eq!(b.state_text(), "Заряжается, до полного 1,0 ч");

        // Две батареи складываются, как это делал DisplayDevice.
        let two = [
            SysBattery {
                status: "Discharging".into(),
                now: Some(30),
                full: Some(100),
                power: Some(30),
                capacity: None,
            },
            SysBattery {
                status: "Discharging".into(),
                now: Some(50),
                full: Some(100),
                power: Some(30),
                capacity: None,
            },
        ];
        let b = sys_battery(&two, None).unwrap();
        assert_eq!(b.percent, 40);
        assert_eq!(b.time.as_deref(), Some("1,3 hours"));

        // Запасной вариант: только процент от ядра.
        let cap = [SysBattery {
            status: "Unknown".into(),
            capacity: Some(77),
            ..SysBattery::default()
        }];
        let b = sys_battery(&cap, None).unwrap();
        assert_eq!(b.percent, 77);
        assert_eq!(b.state, "");
        assert_eq!(b.icon, "battery-level-70-symbolic");
        assert_eq!(b.state_text(), "Состояние неизвестно");

        assert_eq!(sys_battery(&[], None), None);

        // Питание подключено, а аккумулятор ещё пишет «Discharging» или
        // «Not charging» — вилка, не разряд.
        let lag = [SysBattery {
            status: "Discharging".into(),
            now: Some(74),
            full: Some(100),
            power: Some(10),
            capacity: None,
        }];
        let b = sys_battery(&lag, Some(true)).unwrap();
        assert_eq!(b.state, "pending-charge");
        assert_eq!(b.icon, "battery-level-70-plugged-in-symbolic");
        assert_eq!(b.state_text(), "От сети, не заряжается");
        assert_eq!(b.time, None);
        // Отключили — разряд, даже если ядро ещё не обновило состояние.
        let stale = [SysBattery {
            status: "Unknown".into(),
            ..lag[0].clone()
        }];
        assert_eq!(
            sys_battery(&stale, Some(false)).unwrap().state,
            "discharging"
        );
        // Только что отключили, а аккумулятор ещё пишет «Charging» — разряд.
        let unplugged = [SysBattery {
            status: "Charging".into(),
            ..lag[0].clone()
        }];
        assert_eq!(
            sys_battery(&unplugged, Some(false)).unwrap().state,
            "discharging"
        );
        // Заряжается при подключённом питании или когда ядро о питании молчит — молния.
        let ch = [SysBattery {
            status: "Charging".into(),
            ..lag[0].clone()
        }];
        assert_eq!(
            sys_battery(&ch, Some(true)).unwrap().icon,
            "battery-level-70-charging-symbolic"
        );
        let full = [SysBattery {
            status: "Not charging".into(),
            now: Some(100),
            full: Some(100),
            ..SysBattery::default()
        }];
        assert_eq!(
            sys_battery(&full, Some(true)).unwrap().state,
            "fully-charged"
        );
        assert_eq!(sys_battery(&[SysBattery::default()], None), None);
    }

    #[test]
    fn backlight_percent_and_floor() {
        let b = Backlight {
            device: "amdgpu_bl1".into(),
            value: 65535,
            max: 65535,
        };
        assert_eq!(b.percent(), 100);
        assert_eq!(b.value_for(50), 32767);
        // Never fully dark: 0 % is 1 % of the range.
        assert_eq!(b.value_for(0), 655);
        assert_eq!(b.value_for(250), 65535);
        let small = Backlight {
            device: "acpi_video0".into(),
            value: 3,
            max: 7,
        };
        assert_eq!(small.percent(), 43);
        assert_eq!(small.value_for(0), 1);
        assert_eq!(
            Backlight {
                max: 0,
                ..small.clone()
            }
            .percent(),
            0
        );
    }

    #[test]
    fn backlight_device_choice() {
        assert_eq!(pick_backlight(&[("amdgpu_bl1", "raw")]), Some("amdgpu_bl1"));
        assert_eq!(
            pick_backlight(&[
                ("nvidia_0", "raw"),
                ("intel_backlight", "raw"),
                ("acpi_video0", "firmware"),
            ]),
            Some("acpi_video0")
        );
        assert_eq!(
            pick_backlight(&[("nvidia_0", "raw"), ("intel_backlight", "raw")]),
            Some("intel_backlight")
        );
        assert_eq!(pick_backlight(&[("x", "unknown")]), None);
        assert_eq!(pick_backlight(&[]), None);
    }
}
