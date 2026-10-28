//! Проверка окон на собственном Xvfb: рабочий DISPLAY никогда не используется.
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use mimshel_x11::Display;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt, ImageFormat, ImageOrder, MapState, PropMode,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "нужен Xvfb; CI запускает отдельно"]
fn isolated_x11_popup_and_dock() -> Result<()> {
    if std::env::var_os("MIMSHEL_TEST_PRIVATE_X11").is_none() {
        let xvfb = mimshel_core::names::env("TEST_XVFB").unwrap_or_else(|| "Xvfb".into());
        let mut server = Server(
            Command::new(xvfb)
                .args([
                    "-displayfd",
                    "1",
                    "-screen",
                    "0",
                    "800x600x24",
                    "-nolisten",
                    "tcp",
                    "-ac",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()?,
        );
        let stdout = server.0.stdout.take().ok_or("нет stdout Xvfb")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut display = String::new();
            let result = BufReader::new(stdout)
                .read_line(&mut display)
                .map(|_| display);
            let _ = tx.send(result);
        });
        let display: u32 = rx.recv_timeout(Duration::from_secs(5))??.trim().parse()?;
        // Проверяем оба пути передачи картинки — MIT-SHM и обычный PutImage.
        for no_shm in [false, true] {
            let mut child = Command::new("timeout");
            child
                .arg("20s")
                .arg(std::env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "isolated_x11_popup_and_dock",
                    "--nocapture",
                ])
                .env("DISPLAY", format!(":{display}"))
                .env("MIMSHEL_TEST_PRIVATE_X11", "1");
            if no_shm {
                child.env("MIMSHEL_NO_SHM", "1");
            } else {
                child.env_remove("MIMSHEL_NO_SHM");
            }
            assert!(child.status()?.success(), "X11, SHELL_NO_SHM={no_shm}");
        }
        return Ok(());
    }
    let d = Display::connect()?;
    let (observer, _) = RustConnection::connect(None)?;
    let popup = d.create_popup("integration-popup")?;
    for (w, h) in [(320, 200), (160, 90)] {
        popup.show(&d, 10, 20, w, h)?;
        let mut frame =
            tiny_skia::Pixmap::new(u32::from(w), u32::from(h)).ok_or("пустая картинка")?;
        frame.fill(tiny_skia::Color::from_rgba8(0x11, 0x22, 0x33, 255));
        popup.present(&d, &frame)?;
        d.sync()?;
        let geometry = observer.get_geometry(popup.id())?.reply()?;
        assert_eq!(
            (geometry.x, geometry.y, geometry.width, geometry.height),
            (10, 20, w, h)
        );
        assert_eq!(
            observer
                .get_window_attributes(popup.id())?
                .reply()?
                .map_state,
            MapState::VIEWABLE
        );
        let image = observer
            .get_image(ImageFormat::Z_PIXMAP, popup.id(), 0, 0, 1, 1, u32::MAX)?
            .reply()?;
        let bytes: [u8; 4] = image.data[..4].try_into()?;
        let pixel = if observer.setup().image_byte_order == ImageOrder::LSB_FIRST {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        };
        assert_eq!(pixel & 0xffffff, 0x112233);
        popup.hide(&d)?;
        d.sync()?;
        assert_eq!(
            observer
                .get_window_attributes(popup.id())?
                .reply()?
                .map_state,
            MapState::UNMAPPED
        );
    }
    // Виртуальный X-сервер сообщает мониторы через RandR. Если расширение
    // доступно, все возвращённые прямоугольники должны иметь размер.
    for (_, _, w, h) in d.monitors() {
        assert!(w > 0 && h > 0);
    }
    let dock = d.create_dock("integration-dock")?;
    dock.set_geometry(&d, 0, 0, 800, 28)?;
    dock.set_strut_top(&d, 28, 0, 799)?;
    dock.show(&d)?;
    d.sync()?;
    let atom = observer
        .intern_atom(false, b"_NET_WM_STRUT_PARTIAL")?
        .reply()?
        .atom;
    let property = observer
        .get_property(false, dock.id(), atom, AtomEnum::CARDINAL, 0, 12)?
        .reply()?;
    assert_eq!(
        property
            .value32()
            .ok_or("неверный тип strut")?
            .collect::<Vec<_>>(),
        [0, 0, 28, 0, 0, 0, 0, 0, 0, 799, 0, 0]
    );
    // Оконный менеджер может сообщать разные рабочие области для нескольких
    // виртуальных столов. Читаем область активного рабочего стола.
    let work = observer.intern_atom(false, b"_NET_WORKAREA")?.reply()?.atom;
    let current = observer
        .intern_atom(false, b"_NET_CURRENT_DESKTOP")?
        .reply()?
        .atom;
    observer.change_property32(
        PropMode::REPLACE,
        d.root(),
        work,
        AtomEnum::CARDINAL,
        &[0, 28, 800, 572, 0, 28, 800, 472],
    )?;
    observer.change_property32(
        PropMode::REPLACE,
        d.root(),
        current,
        AtomEnum::CARDINAL,
        &[1],
    )?;
    // flush только отправляет запросы: сервер мог ещё не выполнить их, когда
    // придёт чтение по другому соединению. Ответ на запрос — значит, всё
    // отправленное раньше уже выполнено.
    observer.get_input_focus()?.reply()?;
    assert_eq!(d.workarea(), Some((0, 28, 800, 472)));
    Ok(())
}

/// Start a private Xvfb of `size` and run the test `name` again inside it
/// (with `MIMSHEL_TEST_PRIVATE_X11` set). `false` — we are the inner run.
fn outer_run(name: &str, size: &str) -> Result<bool> {
    if std::env::var_os("MIMSHEL_TEST_PRIVATE_X11").is_some() {
        return Ok(false);
    }
    let xvfb = mimshel_core::names::env("TEST_XVFB").unwrap_or_else(|| "Xvfb".into());
    let mut server = Server(
        Command::new(xvfb)
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                size,
                "-nolisten",
                "tcp",
                "-ac",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()?,
    );
    let stdout = server.0.stdout.take().ok_or("нет stdout Xvfb")?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line)?;
    let display: u32 = line.trim().parse()?;
    let status = Command::new("timeout")
        .arg("20s")
        .arg(std::env::current_exe()?)
        .args(["--ignored", "--exact", name, "--nocapture"])
        .env("DISPLAY", format!(":{display}"))
        .env("MIMSHEL_TEST_PRIVATE_X11", "1")
        .status()?;
    assert!(status.success(), "{name} on Xvfb");
    Ok(true)
}

#[test]
#[ignore = "нужен Xvfb; CI запускает отдельно"]
fn isolated_x11_primary_monitor() -> Result<()> {
    use x11rb::protocol::randr::{ConnectionExt as _, MonitorInfo};
    if outer_run("isolated_x11_primary_monitor", "3200x1080x24")? {
        return Ok(());
    }
    let d = std::sync::Arc::new(Display::connect()?);
    d.watch_root()?;
    let (observer, _) = RustConnection::connect(None)?;
    let root = d.root();
    // Events in a thread: the test waits for them with a deadline.
    let (events, seen) = mpsc::channel();
    {
        let d = std::sync::Arc::clone(&d);
        std::thread::spawn(move || {
            while let Ok(ev) = d.next_event() {
                if events.send(ev).is_err() {
                    break;
                }
            }
        });
    }
    let root_changed = || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match seen.recv_timeout(left) {
                Ok(mimshel_x11::Event::Configure { window }) if window == root => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    };
    // A laptop screen on the left and an external one, marked primary, on
    // the right — as `xrandr --setmonitor` would describe them.
    let monitor = |name: &[u8], primary, x, w, h| -> Result<MonitorInfo> {
        Ok(MonitorInfo {
            name: observer.intern_atom(false, name)?.reply()?.atom,
            primary,
            automatic: false,
            x,
            y: 0,
            width: w,
            height: h,
            width_in_millimeters: 300,
            height_in_millimeters: 200,
            outputs: Vec::new(),
        })
    };
    observer.randr_set_monitor(root, monitor(b"LAPTOP", false, 0, 1280, 800)?)?;
    observer.randr_set_monitor(root, monitor(b"EXTERNAL", true, 1280, 1920, 1080)?)?;
    observer.get_input_focus()?.reply()?;
    assert_eq!(d.primary_area(), (1280, 0, 1920, 1080));
    while seen.try_recv().is_ok() {}
    // The primary mark moves to the laptop screen (the monitors are set up
    // anew, as a display change does): the screen size stays the same.
    for name in [&b"LAPTOP"[..], b"EXTERNAL"] {
        let atom = observer.intern_atom(false, name)?.reply()?.atom;
        observer.randr_delete_monitor(root, atom)?;
    }
    observer.randr_set_monitor(root, monitor(b"LAPTOP", true, 0, 1280, 800)?)?;
    observer.randr_set_monitor(root, monitor(b"EXTERNAL", false, 1280, 1920, 1080)?)?;
    observer.get_input_focus()?.reply()?;
    // Xvfb has no CRTCs: it reports a monitor change as ConfigureNotify of
    // the root. Real outputs also send RandR notifications, which
    // `watch_root` subscribes to; both reach the parts as this event.
    assert!(
        root_changed(),
        "a monitor change arrives as Configure of the root"
    );
    assert_eq!(d.primary_area(), (0, 0, 1280, 800));
    Ok(())
}
