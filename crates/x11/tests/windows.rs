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

#[test]
#[ignore = "нужен Xvfb; CI запускает отдельно"]
fn isolated_x11_new_window_first_draw() -> Result<()> {
    use x11rb::protocol::xproto::{
        ConfigureWindowAux, CreateGCAux, CreateWindowAux, Rectangle, WindowClass,
    };
    if outer_run("isolated_x11_new_window_first_draw", "800x600x24")? {
        return Ok(());
    }
    let (app, screen) = RustConnection::connect(None)?;
    let root = app.setup().roots[screen].root;
    // A window that existed before: it is not watched.
    let old = app.generate_id()?;
    app.create_window(
        0,
        old,
        root,
        0,
        0,
        100,
        100,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )?;
    app.map_window(old)?;
    app.get_input_focus()?.reply()?;
    let d = std::sync::Arc::new(Display::connect()?);
    d.watch_root()?;
    d.watch_new_windows()?;
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
    // A program starting up: a window that draws nothing for now (no
    // background, like Chrome before its first frame).
    let window = app.generate_id()?;
    app.create_window(
        0,
        window,
        root,
        0,
        0,
        400,
        300,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )?;
    app.map_window(window)?;
    app.get_input_focus()?.reply()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while d.undrawn_for(window).is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        d.undrawn_for(window).is_some(),
        "the new window waits for its first drawing"
    );
    // Moving some other window must not reach the parts (the root's
    // substructure events are only for catching new windows).
    app.configure_window(old, &ConfigureWindowAux::new().x(50))?;
    app.get_input_focus()?.reply()?;
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(ev) = seen.try_recv() {
        assert!(
            !matches!(ev, mimshel_x11::Event::Configure { window: w } | mimshel_x11::Event::Mapped { window: w } if w == old),
            "root substructure events are dropped: {ev:?}"
        );
    }
    // The program draws: one Drawn, and the window is not waited for any more.
    let gc = app.generate_id()?;
    app.create_gc(gc, window, &CreateGCAux::new().foreground(0x336699))?;
    app.poly_fill_rectangle(
        window,
        gc,
        &[Rectangle {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        }],
    )?;
    app.get_input_focus()?.reply()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut drawn = false;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match seen.recv_timeout(left) {
            Ok(mimshel_x11::Event::Drawn { window: w }) if w == window => {
                drawn = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(drawn, "the first drawing arrives as Event::Drawn");
    assert!(d.undrawn_for(window).is_none());
    Ok(())
}

/// A program dragging files onto our window, speaking XDND on its own
/// connection (as Thunar does).
struct Source {
    conn: RustConnection,
    window: u32,
    atoms: std::collections::HashMap<&'static str, u32>,
}

impl Source {
    fn new() -> Result<Source> {
        use x11rb::protocol::xproto::{CreateWindowAux, WindowClass};
        let (conn, screen) = RustConnection::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )?;
        let mut atoms = std::collections::HashMap::new();
        for name in [
            "XdndEnter",
            "XdndPosition",
            "XdndDrop",
            "XdndStatus",
            "XdndFinished",
            "XdndSelection",
            "XdndActionCopy",
            "XdndActionMove",
            "text/uri-list",
        ] {
            atoms.insert(
                name,
                conn.intern_atom(false, name.as_bytes())?.reply()?.atom,
            );
        }
        conn.set_selection_owner(window, atoms["XdndSelection"], x11rb::CURRENT_TIME)?;
        conn.get_input_focus()?.reply()?;
        Ok(Source {
            conn,
            window,
            atoms,
        })
    }

    fn send(&self, to: u32, kind: &str, data: [u32; 5]) -> Result<()> {
        use x11rb::protocol::xproto::{ClientMessageEvent, EventMask};
        let ev = ClientMessageEvent::new(32, to, self.atoms[kind], data);
        self.conn.send_event(false, to, EventMask::NO_EVENT, ev)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Enter, move to the root point `(x, y)` offering `action`, drop.
    fn drop_at(&self, to: u32, (x, y): (u32, u32), action: &str) -> Result<()> {
        let uri = self.atoms["text/uri-list"];
        self.send(to, "XdndEnter", [self.window, 5 << 24, uri, 0, 0])?;
        let at = (x << 16) | y;
        self.send(
            to,
            "XdndPosition",
            [self.window, 0, at, 0, self.atoms[action]],
        )?;
        self.send(to, "XdndDrop", [self.window, 0, 0, 0, 0])
    }

    /// Answer the target's request for the file list, as the owner of
    /// XdndSelection does; returns once one request was served.
    fn serve(&self, uris: &str, limit: Duration) -> Result<bool> {
        use x11rb::protocol::Event as X;
        use x11rb::protocol::xproto::{EventMask, SelectionNotifyEvent};
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if let Some(X::SelectionRequest(req)) = self.conn.poll_for_event()? {
                self.conn.change_property8(
                    PropMode::REPLACE,
                    req.requestor,
                    req.property,
                    req.target,
                    uris.as_bytes(),
                )?;
                let notify = SelectionNotifyEvent {
                    response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: req.time,
                    requestor: req.requestor,
                    selection: req.selection,
                    target: req.target,
                    property: req.property,
                };
                self.conn
                    .send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
                self.conn.flush()?;
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(false)
    }

    /// The `XdndFinished` the target sent back: (accepted, action).
    fn finished(&self, limit: Duration) -> Result<Option<(bool, u32)>> {
        use x11rb::protocol::Event as X;
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            match self.conn.poll_for_event()? {
                Some(X::ClientMessage(m)) if m.type_ == self.atoms["XdndFinished"] => {
                    let data = m.data.as_data32();
                    return Ok(Some((data[1] & 1 != 0, data[2])));
                }
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        Ok(None)
    }
}

#[test]
#[ignore = "нужен Xvfb; CI запускает отдельно"]
fn isolated_x11_drop_from_another_program() -> Result<()> {
    use mimshel_x11::Event;
    if outer_run("isolated_x11_drop_from_another_program", "800x600x24")? {
        return Ok(());
    }
    let d = std::sync::Arc::new(Display::connect()?);
    let target = d.create_popup("dnd-target")?;
    target.show(&d, 100, 50, 300, 200)?;
    d.enable_drop(target.id())?;
    d.sync()?;
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
    let dropped = || -> Option<(i16, i16, bool, u64)> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match seen.recv_timeout(left) {
                Ok(Event::Dropped {
                    x,
                    y,
                    moving,
                    ticket,
                    ..
                }) => return Some((x, y, moving, ticket)),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    };

    // 1. A normal drop: the point comes in window coordinates, the paths
    //    by ticket, and the source hears the result only at finish_drop.
    let source = Source::new()?;
    source.drop_at(target.id(), (140, 90), "XdndActionMove")?;
    assert!(source.serve(
        "file:///home/u/a.txt\r\nfile:///home/u/b%20c.txt\r\n",
        Duration::from_secs(3)
    )?);
    let (x, y, moving, ticket) = dropped().ok_or("no Dropped event")?;
    assert_eq!(
        (x, y),
        (40, 40),
        "root (140, 90) in the window at (100, 50)"
    );
    assert!(moving, "the source offered a move");
    assert_eq!(
        d.take_drop(ticket),
        [
            std::path::PathBuf::from("/home/u/a.txt"),
            std::path::PathBuf::from("/home/u/b c.txt")
        ]
    );
    assert_eq!(source.finished(Duration::from_millis(200))?, None);
    d.finish_drop(ticket, true);
    let (accepted, action) = source
        .finished(Duration::from_secs(3))?
        .ok_or("no XdndFinished")?;
    assert!(accepted);
    assert_eq!(action, source.atoms["XdndActionMove"]);

    // 2. A source that hangs after XdndDrop never hands over the list (one
    //    that dies is fine: the server answers for it). The next drop is
    //    refused while the first may still come, and accepted after the
    //    wait (it used to be refused until the service restarted).
    let hung = Source::new()?;
    hung.drop_at(target.id(), (140, 90), "XdndActionCopy")?;
    let next = Source::new()?;
    next.drop_at(target.id(), (150, 100), "XdndActionCopy")?;
    let (refused, _) = next
        .finished(Duration::from_secs(2))?
        .ok_or("the second drop gets an answer")?;
    assert!(!refused, "refused while the first drop may still come");
    std::thread::sleep(Duration::from_secs(5));
    next.drop_at(target.id(), (150, 100), "XdndActionCopy")?;
    assert!(next.serve("file:///tmp/x\r\n", Duration::from_secs(3))?);
    let (_, _, moving, ticket) = dropped().ok_or("a drop after the wait works")?;
    assert!(!moving);
    assert_eq!(d.take_drop(ticket), [std::path::PathBuf::from("/tmp/x")]);
    Ok(())
}
