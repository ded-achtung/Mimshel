//! Звук, батарея и «Не беспокоить» для строки меню — через программы
//! системы: `pactl` (PipeWire/PulseAudio), `upower`, `xfconf-query`
//! (уведомления Xfce). Каждая следит за изменениями сама (`pactl subscribe`,
//! `upower --monitor`, `xfconf-query -m`), а мы читаем состояние заново и
//! присылаем его службе.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use shell_menubar::status::{Battery, Sound, parse_mute, parse_sinks, parse_upower, parse_volume};

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
    let volume = parse_volume(&output("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])?)?;
    Some(Sound {
        volume,
        muted: parse_mute(&output("pactl", &["get-sink-mute", "@DEFAULT_SINK@"])?),
        sinks: parse_sinks(&output("pactl", &["list", "sinks"]).unwrap_or_default()),
        default: output("pactl", &["get-default-sink"])
            .unwrap_or_default()
            .trim()
            .to_string(),
    })
}

pub fn read_battery() -> Option<Battery> {
    // Сводное устройство есть и без батареи — сначала смотрим, есть ли она.
    let devices = output("upower", &["-e"])?;
    if !devices.lines().any(|d| d.contains("/battery_")) {
        return None;
    }
    parse_upower(&output(
        "upower",
        &["-i", "/org/freedesktop/UPower/devices/DisplayDevice"],
    )?)
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
pub fn read_network() -> shell_menubar::Network {
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
    shell_menubar::Network {
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
        |l| l.contains(" sink ") || l.contains(" server "),
        read_sound,
        Msg::Sound,
        tx,
    );
}

pub fn watch_battery(tx: Sender<Msg>) {
    watch(
        "upower",
        &["--monitor"],
        |_| true,
        read_battery,
        Msg::Battery,
        tx,
    );
}

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
}
