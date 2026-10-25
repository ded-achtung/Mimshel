//! Звук, батарея и «Не беспокоить» для строки меню. Звук — через `pactl`
//! (PipeWire/PulseAudio): одно обновление — два коротких запроса, изменения
//! ловит `pactl subscribe`. Батарея — напрямую из файлов ядра
//! `/sys/class/power_supply` (то же, что читает UPower, но без демона и без
//! разбора текста): по событиям ядра (`udevadm monitor`), по смене
//! подключения питания (сверка раз в секунду) и раз в 30 с; сообщение —
//! только при изменении.
//! «Не беспокоить» — `xfconf-query` (уведомления Xfce), изменения ловит
//! `xfconf-query -m`.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use mimshel_menubar::status::{
    Battery, Sound, SysBattery, parse_app_sounds, parse_default_state, parse_sinks, sys_battery,
};

use crate::ui::Msg;

/// Ограничение относится к коротким запросам, но не к наблюдателям.
/// timeout завершает и группу дочерних процессов; KILL страхует зависший TERM.
fn bounded_command(program: &str, limit: &str) -> Command {
    let mut cmd = Command::new("timeout");
    cmd.args(["--signal=TERM", "--kill-after=1s", limit, program]);
    cmd
}

/// Вывод команды (с `LC_ALL=C` — ответы на одном языке) или `None`.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = bounded_command(program, "3s")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run(program: &str, args: &[&str]) {
    let _ = bounded_command(program, "3s")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn read_sound() -> Option<Sound> {
    // Всё нужное есть в одном выводе `pactl list sinks`: громкость и mute
    // устройства по умолчанию берутся из него же, вторым запросом — только
    // имя устройства по умолчанию.
    let list = output("pactl", &["list", "sinks"])?;
    let default = output("pactl", &["get-default-sink"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let (volume, muted) = parse_default_state(&list, &default)?;
    let apps = output("pactl", &["list", "sink-inputs"])
        .map(|t| parse_app_sounds(&t))
        .unwrap_or_default();
    Some(Sound {
        volume,
        muted,
        sinks: parse_sinks(&list),
        default,
        apps,
    })
}

const POWER_SUPPLY: &str = "/sys/class/power_supply";

pub fn read_battery() -> Option<Battery> {
    let root = Path::new(POWER_SUPPLY);
    sys_battery(&batteries_in(root), on_line_power(root))
}

/// Подключено ли питание: есть ли среди источников (блок питания, USB) хоть
/// один с `online` = 1. Таких источников нет — `None`.
fn on_line_power(root: &Path) -> Option<bool> {
    let mut found = None;
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let d = e.path();
        let text = |f: &str| std::fs::read_to_string(d.join(f)).ok();
        if text("type").is_none_or(|t| t.trim() == "Battery") {
            continue;
        }
        let Some(online) = text("online") else {
            continue;
        };
        found = Some(found.unwrap_or(false) || online.trim() == "1");
    }
    found
}

/// Что меняется сразу при подключении и отключении зарядки: `online` блоков
/// питания и `status` батарей. Читать — несколько крошечных файлов.
fn power_signature(root: &Path) -> String {
    let mut sig = String::new();
    if let Ok(dir) = std::fs::read_dir(root) {
        let mut dirs: Vec<_> = dir.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            for f in ["online", "status"] {
                if let Ok(v) = std::fs::read_to_string(d.join(f)) {
                    sig.push_str(v.trim());
                    sig.push('|');
                }
            }
        }
    }
    sig
}

/// Батареи из каталога ядра: `type` = Battery, `present` ≠ 0. Заряд ядро
/// отдаёт энергией (мкВт·ч) или зарядом (мкА·ч) — берём что есть, мощность
/// идёт из той же семьи (`power_now` или `current_now`).
fn batteries_in(root: &Path) -> Vec<SysBattery> {
    let Ok(dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| {
            let d = e.path();
            let text = |f: &str| std::fs::read_to_string(d.join(f)).ok();
            let num = |f: &str| text(f).and_then(|s| s.trim().parse::<u64>().ok());
            if text("type").is_none_or(|t| t.trim() != "Battery") {
                return None;
            }
            if text("present").is_some_and(|p| p.trim() == "0") {
                return None;
            }
            Some(SysBattery {
                status: text("status").unwrap_or_default().trim().to_string(),
                now: num("energy_now").or_else(|| num("charge_now")),
                full: num("energy_full")
                    .or_else(|| num("charge_full"))
                    .or_else(|| num("charge_full_design")),
                power: num("power_now").or_else(|| num("current_now")),
                capacity: num("capacity"),
            })
        })
        .collect()
}

pub fn read_dnd() -> bool {
    output(
        "xfconf-query",
        &["-c", "xfce4-notifyd", "-p", "/do-not-disturb"],
    )
    .is_some_and(|v| v.trim() == "true")
}

pub fn set_dnd(on: bool) {
    let value = if on { "true" } else { "false" };
    // -n -t bool — создать, если настройки ещё нет.
    run(
        "xfconf-query",
        &[
            "-c",
            "xfce4-notifyd",
            "-p",
            "/do-not-disturb",
            "-n",
            "-t",
            "bool",
            "-s",
            value,
        ],
    );
}

/// Сеть: подключение (`nmcli device`) и есть ли Wi-Fi (`nmcli radio`).
pub fn read_network() -> mimshel_menubar::Network {
    let devices =
        output("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]).unwrap_or_default();
    let active = devices.lines().find_map(|l| {
        let mut f = l.splitn(3, ':');
        let (kind, state, name) = (f.next()?, f.next()?, f.next()?);
        (matches!(kind, "ethernet" | "wifi") && state == "connected")
            .then(|| (kind == "ethernet", name.to_string()))
    });
    let radio = output("nmcli", &["-t", "-f", "WIFI-HW,WIFI", "radio"]).unwrap_or_default();
    let mut r = radio.trim().split(':');
    let wifi = match (r.next(), r.next()) {
        (Some("enabled"), Some(state)) => Some(state == "enabled"),
        _ => None,
    };
    let (wired, connection) = active.unwrap_or_default();
    mimshel_menubar::Network {
        connection,
        wired,
        wifi,
    }
}

/// Bluetooth: `None` — адаптера нет; иначе включён ли (по rfkill в /sys —
/// мгновенно; `bluetoothctl` без службы bluetooth зависает).
pub fn read_bluetooth() -> Option<bool> {
    let adapters = std::fs::read_dir("/sys/class/bluetooth").ok()?.count();
    if adapters == 0 {
        return None;
    }
    let blocked = std::fs::read_dir("/sys/class/rfkill")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            std::fs::read_to_string(e.path().join("type")).is_ok_and(|t| t.trim() == "bluetooth")
        })
        .any(|e| std::fs::read_to_string(e.path().join("soft")).is_ok_and(|v| v.trim() == "1"));
    Some(!blocked)
}

/// Последовательные запросы состояния и изменения сети — вне потока интерфейса.
pub enum ControlRequest {
    Refresh,
    Wifi(bool),
    Bluetooth(bool),
}

pub fn control_worker(tx: Sender<Msg>) -> mpsc::SyncSender<ControlRequest> {
    let (request, rx) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        loop {
            let network = read_network();
            let bluetooth = read_bluetooth();
            if tx.send(Msg::Control(network, bluetooth)).is_err() {
                return;
            }
            match rx.recv() {
                Ok(ControlRequest::Refresh) => {}
                Ok(ControlRequest::Wifi(on)) => {
                    run("nmcli", &["radio", "wifi", if on { "on" } else { "off" }])
                }
                Ok(ControlRequest::Bluetooth(on)) => {
                    run("bluetoothctl", &["power", if on { "on" } else { "off" }])
                }
                Err(_) => return,
            }
        }
    });
    request
}

/// Что сделать со звуком.
pub enum Audio {
    Refresh,
    Volume(u32),
    /// Громкость программы: все её потоки и проценты.
    AppVolume(Vec<u32>, u32),
    ToggleMute,
    Sink(String),
}

/// Поток, который меняет звук. Пока тащат ползунок, приходит много
/// громкостей подряд — выполняем только последнюю из накопившихся.
pub fn audio_worker(events: Sender<Msg>) -> Sender<Audio> {
    let (tx, rx) = mpsc::channel::<Audio>();
    std::thread::spawn(move || {
        while let Ok(mut cmd) = rx.recv() {
            while let Ok(next) = rx.try_recv() {
                match (&cmd, &next) {
                    (Audio::Volume(_), Audio::Volume(_)) => cmd = next,
                    (Audio::AppVolume(a, _), Audio::AppVolume(b, _)) if a == b => cmd = next,
                    _ => {
                        apply(cmd, &events);
                        cmd = next;
                    }
                }
            }
            apply(cmd, &events);
        }
    });
    tx
}

fn apply(cmd: Audio, tx: &Sender<Msg>) {
    match cmd {
        Audio::Refresh => {
            let _ = tx.send(Msg::Sound(read_sound()));
        }
        Audio::Volume(v) => {
            run("pactl", &["set-sink-mute", "@DEFAULT_SINK@", "0"]);
            run(
                "pactl",
                &["set-sink-volume", "@DEFAULT_SINK@", &format!("{v}%")],
            );
        }
        Audio::AppVolume(inputs, v) => {
            for i in inputs {
                let i = i.to_string();
                run("pactl", &["set-sink-input-mute", &i, "0"]);
                run("pactl", &["set-sink-input-volume", &i, &format!("{v}%")]);
            }
        }
        Audio::ToggleMute => run("pactl", &["set-sink-mute", "@DEFAULT_SINK@", "toggle"]),
        Audio::Sink(name) => run("pactl", &["set-default-sink", &name]),
    }
}

/// Следить за программой-наблюдателем (`args`) и на каждое её сообщение
/// (пачкой, после затишья 100 мс) присылать `read()`. Наблюдатель вышел
/// (например, перезапустили звук) — запустить снова через 2 с.
fn watch<T: Send + 'static>(
    program: &'static str,
    args: &'static [&'static str],
    relevant: fn(&str) -> bool,
    read: fn() -> T,
    msg: fn(T) -> Msg,
    tx: Sender<Msg>,
) {
    let (ping_tx, ping_rx) = mpsc::channel::<()>();
    spawn_watcher(program, args, relevant, ping_tx);
    std::thread::spawn(move || {
        if tx.send(msg(read())).is_err() {
            return;
        }
        while ping_rx.recv().is_ok() {
            while ping_rx.recv_timeout(Duration::from_millis(100)).is_ok() {}
            if tx.send(msg(read())).is_err() {
                return;
            }
        }
    });
}

/// Держит программу-наблюдателя запущенной (упала — запустить снова через
/// 2 с) и на каждую её важную строку посылает `ping`.
fn spawn_watcher(
    program: &'static str,
    args: &'static [&'static str],
    relevant: fn(&str) -> bool,
    ping_tx: Sender<()>,
) {
    std::thread::spawn(move || {
        loop {
            let child = watcher_command(program)
                .args(args)
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = child {
                if let Some(out) = child.stdout.take() {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if relevant(&line) && ping_tx.send(()).is_err() {
                            return;
                        }
                    }
                }
                let _ = child.wait();
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Команда для наблюдателя. Наблюдатель пишет только при событиях и
/// поэтому не замечает, что службы не стало, — после каждого перезапуска
/// службы такие копии оставались висеть. `setpriv --pdeathsig TERM` просит
/// ядро завершить его вместе со службой (точнее — с потоком, который его
/// запустил; этот поток живёт, пока жива служба).
fn watcher_command(program: &str) -> Command {
    if std::path::Path::new("/usr/bin/setpriv").exists() {
        let mut cmd = Command::new("/usr/bin/setpriv");
        cmd.args(["--pdeathsig", "TERM", "--", program]);
        cmd
    } else {
        Command::new(program)
    }
}

pub fn watch_sound(tx: Sender<Msg>) {
    watch(
        "pactl",
        &["subscribe"],
        // `sink-input` — программа начала или кончила играть, сменила громкость.
        |l| l.contains(" sink ") || l.contains(" sink-input ") || l.contains(" server "),
        read_sound,
        Msg::Sound,
        tx,
    );
}

/// Батарея — из файлов ядра. Когда перечитывать: по событию ядра
/// (`udevadm monitor` — подключили или отключили зарядку) сразу; там, где
/// событий нет (в VirtualBox их нет), раз в секунду сверяются только
/// `online` и `status` — подключение видно через секунду; проценты меняются
/// медленно и перечитываются раз в 30 с. Службе уходит только то, что
/// изменилось.
pub fn watch_battery(tx: Sender<Msg>) {
    let (ping_tx, ping_rx) = mpsc::channel::<()>();
    spawn_watcher(
        "udevadm",
        &["monitor", "--kernel", "--subsystem-match=power_supply"],
        |l| l.starts_with("KERNEL["),
        ping_tx,
    );
    std::thread::spawn(move || {
        let root = Path::new(POWER_SUPPLY);
        let mut last = read_battery();
        let mut sig = power_signature(root);
        let mut since_full = std::time::Instant::now();
        if tx.send(Msg::Battery(last.clone())).is_err() {
            return;
        }
        loop {
            let changed = match ping_rx.recv_timeout(POWER_CHECK) {
                Ok(()) => {
                    // При отключении событий приходит несколько подряд.
                    while ping_rx.recv_timeout(Duration::from_millis(100)).is_ok() {}
                    true
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now = power_signature(root);
                    let moved = now != sig;
                    sig = now;
                    moved || since_full.elapsed() >= BATTERY_POLL
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            if !changed {
                continue;
            }
            since_full = std::time::Instant::now();
            sig = power_signature(root);
            let now = read_battery();
            if now != last {
                last = now.clone();
                if tx.send(Msg::Battery(now)).is_err() {
                    return;
                }
            }
        }
    });
}

/// Как часто сверять, подключено ли питание (если ядро не прислало событие).
const POWER_CHECK: Duration = Duration::from_secs(1);
/// Как часто перечитывать батарею целиком (ради процентов).
const BATTERY_POLL: Duration = Duration::from_secs(30);

pub fn watch_dnd(tx: Sender<Msg>) {
    watch(
        "xfconf-query",
        &["-c", "xfce4-notifyd", "-m"],
        |l| l.contains("do-not-disturb"),
        read_dnd,
        Msg::Dnd,
        tx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_timeout_and_output() {
        let out = bounded_command("printf", "1s")
            .args(["%s", "готово"])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "готово");
        // Дочерний процесс игнорирует TERM: ограничитель обязан применить KILL.
        let start = std::time::Instant::now();
        let out = bounded_command("sh", "0.1s")
            .args(["-c", "trap '' TERM; sleep 30"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn batteries_from_sys_fixture() {
        let root = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/power_supply"
        ));
        assert_eq!(on_line_power(root), Some(true), "AC: online = 1");
        assert!(power_signature(root).contains("Discharging"));
        let bats = batteries_in(root);
        // BAT0 — батарея; AC (Mains) и BAT1 (present: 0) — нет.
        assert_eq!(bats.len(), 1);
        assert_eq!(bats[0].status, "Discharging");
        assert_eq!(bats[0].now, Some(2_050_000));
        assert_eq!(bats[0].full, Some(5_000_000));
        assert_eq!(bats[0].power, Some(800_000));
        let b = sys_battery(&bats, None).unwrap();
        assert_eq!(b.percent, 41);
        assert_eq!(b.icon, "battery-level-40-symbolic");

        // Семья «заряд/ток» вместо «энергия/мощность», файла present нет.
        let root = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/power_supply_charge"
        ));
        let bats = batteries_in(root);
        assert_eq!(bats.len(), 1);
        assert_eq!(bats[0].now, Some(1_500_000));
        assert_eq!(bats[0].full, Some(3_000_000));
        let b = sys_battery(&bats, None).unwrap();
        assert_eq!(b.percent, 50);
        assert_eq!(b.state, "charging");
    }
}
