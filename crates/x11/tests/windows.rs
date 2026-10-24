//! Проверка окон на собственном Xvfb: рабочий DISPLAY никогда не используется.
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use shell_x11::Display;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, ImageFormat, ImageOrder, MapState, PropMode};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

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
    if std::env::var_os("SHELL_TEST_PRIVATE_X11").is_none() {
        let xvfb = std::env::var_os("SHELL_TEST_XVFB").unwrap_or_else(|| "Xvfb".into());
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
                .env("SHELL_TEST_PRIVATE_X11", "1");
            if no_shm {
                child.env("SHELL_NO_SHM", "1");
            } else {
                child.env_remove("SHELL_NO_SHM");
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
    observer.flush()?;
    assert_eq!(d.workarea(), Some((0, 28, 800, 472)));
    Ok(())
}
