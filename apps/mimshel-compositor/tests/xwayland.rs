//! Real XWayland integration without a GPU or host display.
#[path = "support/input.rs"]
mod input;
#[path = "support/x11_lists.rs"]
mod lists;
use mimshel_compositor::{backend::headless::Server, model::Mode};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{self, AtomEnum, ConnectionExt, CreateWindowAux, PropMode},
    wrapper::ConnectionExt as _,
};

fn pump(server: &mut Server, ready: impl Fn(&Server) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready(server) {
        assert!(Instant::now() < deadline, "XWayland integration timed out");
        server.dispatch(Some(Duration::from_millis(10))).unwrap();
    }
}

#[test]
fn real_x11_window_policy_and_cleanup() {
    if std::process::Command::new("Xwayland")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("SKIP: Xwayland is missing from PATH");
        return;
    }
    let runtime = tempfile::tempdir().unwrap();
    let mut server = Server::private(&runtime.path().join("wayland-test")).unwrap();
    let display = server.start_xwayland().unwrap();
    let socket = format!("/tmp/.X11-unix/X{display}");
    let lock = format!("/tmp/.X{display}-lock");
    assert!(std::path::Path::new(&socket).exists());
    assert!(std::path::Path::new(&lock).exists());
    let (sender, receiver) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let client = std::thread::spawn(move || {
        let (connection, screen) = x11rb::connect(Some(&format!(":{display}"))).unwrap();
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                320,
                200,
                0,
                xproto::WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().background_pixel(0x34759a),
            )
            .unwrap();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"X11 integration",
            )
            .unwrap();
        let name = connection
            .intern_atom(false, b"_NET_WM_NAME")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let utf8 = connection
            .intern_atom(false, b"UTF8_STRING")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        connection
            .change_property8(PropMode::REPLACE, window, name, utf8, b"XWayland title")
            .unwrap();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                b"test\0XWaylandTest\0",
            )
            .unwrap();
        let mut hints = x11rb::properties::WmSizeHints::new();
        hints.min_size = Some((160, 100));
        hints.max_size = Some((700, 500));
        hints.set_normal_hints(&connection, window).unwrap();
        connection.map_window(window).unwrap();
        connection.flush().unwrap();
        sender.send(window).unwrap();
        loop {
            match stopped.recv_timeout(Duration::from_secs(30)).unwrap() {
                "fullscreen" => {
                    let state = connection
                        .intern_atom(false, b"_NET_WM_STATE")
                        .unwrap()
                        .reply()
                        .unwrap()
                        .atom;
                    let fullscreen = connection
                        .intern_atom(false, b"_NET_WM_STATE_FULLSCREEN")
                        .unwrap()
                        .reply()
                        .unwrap()
                        .atom;
                    let event = xproto::ClientMessageEvent::new(
                        32,
                        window,
                        state,
                        [1, fullscreen, 0, 1, 0],
                    );
                    connection
                        .send_event(
                            false,
                            root,
                            xproto::EventMask::SUBSTRUCTURE_REDIRECT
                                | xproto::EventMask::SUBSTRUCTURE_NOTIFY,
                            event,
                        )
                        .unwrap();
                }
                "override" => {
                    let menu = connection.generate_id().unwrap();
                    connection
                        .create_window(
                            x11rb::COPY_DEPTH_FROM_PARENT,
                            menu,
                            root,
                            850,
                            100,
                            90,
                            60,
                            0,
                            xproto::WindowClass::INPUT_OUTPUT,
                            0,
                            &CreateWindowAux::new()
                                .override_redirect(1)
                                .background_pixel(0xffffff),
                        )
                        .unwrap();
                    connection.map_window(menu).unwrap();
                }
                "title" => {
                    connection
                        .change_property8(PropMode::REPLACE, window, name, utf8, b"Changed title")
                        .unwrap();
                }
                "quit" => break,
                _ => panic!("unknown client action"),
            }
            connection.flush().unwrap();
        }
        connection.destroy_window(window).unwrap();
        connection.flush().unwrap();
    });
    pump(&mut server, |s| {
        s.state.windows.len() == 1
            && s.state.windows[0]
                .1
                .x11_surface()
                .is_some_and(|x| x.wl_surface().is_some())
    });
    let window = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    let id = server.state.windows[0].0;
    assert_eq!(server.state.model.focus, Some(id));
    assert_eq!(server.state.model.windows[0].x11_title, "XWayland title");
    assert_eq!(
        server.state.windows[0].1.x11_surface().unwrap().class(),
        "XWaylandTest"
    );
    let path = runtime.path().join("wayland-test");
    let (listed, listing) = mpsc::channel();
    let observer = std::thread::spawn(move || {
        lists::check(path, "XWayland title");
        listed.send(()).unwrap();
    });
    pump(&mut server, |_| listing.try_recv().is_ok());
    observer.join().unwrap();
    server.state.window_mode(id, Mode::Maximized);
    let rect = server.state.model.rect(id).unwrap();
    assert_eq!((rect.width, rect.height), (700, 500));
    assert_eq!(server.state.model.windows[0].mode, Mode::Maximized);
    server.state.window_mode(id, Mode::Minimized);
    assert_eq!(server.state.model.focus, None);
    server.state.model.restore(id);
    server.state.configure_windows();
    assert_eq!(server.state.model.focus, Some(id));
    stop.send("fullscreen").unwrap();
    pump(&mut server, |s| {
        s.state.model.windows[0].mode == Mode::Fullscreen
    });
    assert_eq!(server.state.model.rect(id), Some(server.state.model.output));
    server.state.window_mode(id, Mode::Normal);
    assert_eq!(server.state.model.rect(id).unwrap().width, 320);
    assert_eq!(
        server.state.windows[0].1.x11_surface().unwrap().window_id(),
        window
    );
    stop.send("title").unwrap();
    pump(&mut server, |s| {
        s.state.model.windows[0].x11_title == "Changed title"
    });
    stop.send("override").unwrap();
    pump(&mut server, |s| {
        s.state.overrides.len() == 1
            && s.state.overrides[0]
                .x11_surface()
                .is_some_and(|x| x.wl_surface().is_some())
    });
    assert_eq!(server.state.model.windows.len(), 1);
    assert_eq!(server.state.model.focus, Some(id));
    assert_eq!(
        server.state.overrides[0]
            .x11_surface()
            .unwrap()
            .geometry()
            .loc,
        (850, 100).into()
    );
    server.state.pointer = (860.0, 110.0).into();
    input::click(&mut server.state);
    assert_eq!(
        server.state.model.focus,
        Some(id),
        "override redirected focus"
    );
    assert!(server.state.drag.is_none());
    server.state.model.focus = None;
    server.state.sync_focus();
    server.state.pointer = (20.0, 50.0).into();
    input::click(&mut server.state);
    assert_eq!(
        server.state.model.focus,
        Some(id),
        "click did not focus X11 window"
    );
    let path = runtime.path().join("wayland-test");
    let (listed, listing) = mpsc::channel();
    let observer = std::thread::spawn(move || {
        lists::check(path, "Changed title");
        listed.send(()).unwrap();
    });
    pump(&mut server, |_| listing.try_recv().is_ok());
    observer.join().unwrap();
    stop.send("quit").unwrap();
    client.join().unwrap();
    pump(&mut server, |s| s.state.windows.is_empty());
    drop(server);
    assert!(
        !runtime.path().join("wayland-test").exists(),
        "Wayland socket leaked"
    );
    assert!(
        !runtime.path().join("wayland-test.lock").exists(),
        "Wayland lock leaked"
    );
    assert!(!std::path::Path::new(&socket).exists(), "X socket leaked");
    assert!(!std::path::Path::new(&lock).exists(), "X lock leaked");
}
