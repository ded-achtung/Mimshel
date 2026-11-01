//! Selection and drag offers over independent, real Wayland connections.
#[allow(dead_code)]
#[path = "support/input.rs"]
mod input;
use mimshel_compositor::backend::headless::Server;
use std::{
    io::{Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    sync::mpsc,
    time::Duration,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop, protocol::*};
use wayland_protocols::{
    wp::primary_selection::zv1::client::{
        zwp_primary_selection_device_manager_v1 as pm, zwp_primary_selection_device_v1 as pd,
        zwp_primary_selection_offer_v1 as po, zwp_primary_selection_source_v1 as ps,
    },
    xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
};
#[derive(Default)]
struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    shell: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    primary: Option<pm::ZwpPrimarySelectionDeviceManagerV1>,
    offer: Option<wl_data_offer::WlDataOffer>,
    primary_offer: Option<po::ZwpPrimarySelectionOfferV1>,
    mimes: Vec<String>,
    primary_mimes: Vec<String>,
    drag: bool,
    serial: u32,
    group: u32,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        s: &mut Self,
        r: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = e
        {
            match interface.as_str() {
                "wl_compositor" => s.compositor = Some(r.bind(name, version.min(4), q, ())),
                "wl_shm" => s.shm = Some(r.bind(name, 1, q, ())),
                "wl_seat" => s.seat = Some(r.bind(name, 7, q, ())),
                "xdg_wm_base" => s.shell = Some(r.bind(name, 3, q, ())),
                "wl_data_device_manager" => s.manager = Some(r.bind(name, 3, q, ())),
                "zwp_primary_selection_device_manager_v1" => {
                    s.primary = Some(r.bind(name, 1, q, ()))
                }
                _ => {}
            }
        }
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        p: &xdg_wm_base::XdgWmBase,
        e: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = e {
            p.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        _: &mut Self,
        p: &xdg_surface::XdgSurface,
        e: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = e {
            p.ack_configure(serial);
        }
    }
}
impl Dispatch<wl_data_device::WlDataDevice, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_data_device::WlDataDevice,
        e: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_data_device::Event::Selection { id } => s.offer = id,
            wl_data_device::Event::Enter { id, serial, .. } => {
                s.serial = serial;
                s.offer = id;
                s.drag = true;
            }
            _ => {}
        }
    }
    wayland_client::event_created_child!(Client, wl_data_device::WlDataDevice, [0 => (wl_data_offer::WlDataOffer, ())]);
}
impl Dispatch<wl_data_offer::WlDataOffer, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        e: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = e {
            s.mimes.push(mime_type);
        }
    }
}
impl Dispatch<pd::ZwpPrimarySelectionDeviceV1, ()> for Client {
    fn event(
        s: &mut Self,
        _: &pd::ZwpPrimarySelectionDeviceV1,
        e: pd::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let pd::Event::Selection { id } = e {
            s.primary_offer = id;
        }
    }
    wayland_client::event_created_child!(Client, pd::ZwpPrimarySelectionDeviceV1, [0 => (po::ZwpPrimarySelectionOfferV1, ())]);
}
impl Dispatch<po::ZwpPrimarySelectionOfferV1, ()> for Client {
    fn event(
        s: &mut Self,
        _: &po::ZwpPrimarySelectionOfferV1,
        e: po::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let po::Event::Offer { mime_type } = e {
            s.primary_mimes.push(mime_type);
        }
    }
}
impl Dispatch<wl_data_source::WlDataSource, ()> for Client {
    fn event(
        _: &mut Self,
        _: &wl_data_source::WlDataSource,
        e: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_source::Event::Send { fd, .. } = e {
            std::fs::File::from(fd)
                .write_all(b"file:///tmp/selection-test\r\n")
                .unwrap();
        }
    }
}
impl Dispatch<ps::ZwpPrimarySelectionSourceV1, ()> for Client {
    fn event(
        _: &mut Self,
        _: &ps::ZwpPrimarySelectionSourceV1,
        e: ps::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ps::Event::Send { fd, .. } = e {
            std::fs::File::from(fd).write_all(b"primary text").unwrap();
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_pointer::WlPointer,
        e: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button { serial, .. } = e {
            s.serial = serial;
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        e: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Modifiers { group, .. } = e {
            s.group = group;
        }
    }
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore wl_buffer::WlBuffer);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_seat::WlSeat);
delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);
delegate_noop!(Client: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(Client: ignore pm::ZwpPrimarySelectionDeviceManagerV1);
struct Peer {
    client: Client,
    queue: EventQueue<Client>,
    surface: wl_surface::WlSurface,
    data: wl_data_device::WlDataDevice,
    primary: pd::ZwpPrimarySelectionDeviceV1,
}
impl Peer {
    fn new(path: &std::path::Path) -> Self {
        let connection = Connection::from_socket(UnixStream::connect(path).unwrap()).unwrap();
        let mut queue = connection.new_event_queue();
        let q = queue.handle();
        connection.display().get_registry(&q, ());
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let seat = client.seat.as_ref().unwrap();
        seat.get_pointer(&q, ());
        seat.get_keyboard(&q, ());
        let data = client
            .manager
            .as_ref()
            .unwrap()
            .get_data_device(seat, &q, ());
        let primary = client.primary.as_ref().unwrap().get_device(seat, &q, ());
        let surface = client.compositor.as_ref().unwrap().create_surface(&q, ());
        let xdg = client
            .shell
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &q, ());
        xdg.get_toplevel(&q, ());
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        let file = tempfile::tempfile().unwrap();
        file.set_len(100 * 80 * 4).unwrap();
        let pool = client
            .shm
            .as_ref()
            .unwrap()
            .create_pool(file.as_fd(), 100 * 80 * 4, &q, ());
        let buffer = pool.create_buffer(0, 100, 80, 400, wl_shm::Format::Argb8888, &q, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        Self {
            client,
            queue,
            surface,
            data,
            primary,
        }
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.client).unwrap();
    }
    fn copy(&mut self) {
        let q = self.queue.handle();
        let source = self
            .client
            .manager
            .as_ref()
            .unwrap()
            .create_data_source(&q, ());
        source.offer("text/uri-list".into());
        self.data.set_selection(Some(&source), 0);
        let primary = self.client.primary.as_ref().unwrap().create_source(&q, ());
        primary.offer("text/plain".into());
        self.primary.set_selection(Some(&primary), 0);
        self.sync();
    }
}
enum Action {
    Focus(usize),
    DragStart,
    DragTarget,
    Press(usize),
    Target(usize),
    Release,
    IconPresent,
    Key(u32, bool),
    Gesture(bool),
    Xwayland,
    Count,
    Stop,
}
struct Harness {
    tx: mpsc::Sender<Action>,
    ack: mpsc::Receiver<u32>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Harness {
    fn new(path: std::path::PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let (reply, ack) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut server = Server::private(&path).unwrap();
            reply.send(0).unwrap();
            // Deterministic test keymap; production reads the enclosing session.
            server
                .state
                .keyboard
                .clone()
                .set_xkb_config(
                    &mut server.state,
                    smithay::input::keyboard::XkbConfig {
                        layout: "us,ru",
                        options: Some("grp:alt_shift_toggle".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            loop {
                server.dispatch(Some(Duration::from_millis(2))).unwrap();
                if let Ok(action) = rx.try_recv() {
                    let state = &mut server.state;
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    let mut result = 0;
                    match action {
                        Action::Stop => break,
                        Action::Gesture(super_drag) => {
                            let record = &state.model.windows[0];
                            let point = if super_drag {
                                (record.rect.x + 40, record.rect.y + 50)
                            } else {
                                (record.rect.x + 80, record.rect.y + 10)
                            };
                            state.pointer = (f64::from(point.0), f64::from(point.1)).into();
                            state.pointer_inside = true;
                            if super_drag {
                                state.keyboard.clone().input::<(), _>(
                                    state,
                                    133_u32.into(),
                                    smithay::backend::input::KeyState::Pressed,
                                    serial,
                                    1,
                                    |_, _, _| smithay::input::keyboard::FilterResult::Forward,
                                );
                            }
                            state.input::<input::TestInput>(
                                smithay::backend::input::InputEvent::PointerButton {
                                    event: input::Button(
                                        smithay::backend::input::ButtonState::Pressed,
                                    ),
                                },
                                (1280, 800).into(),
                            );
                            result = u32::from(state.pointer_handle.is_grabbed());
                            state.input::<input::TestInput>(
                                smithay::backend::input::InputEvent::PointerButton {
                                    event: input::Button(
                                        smithay::backend::input::ButtonState::Released,
                                    ),
                                },
                                (1280, 800).into(),
                            );
                            if super_drag {
                                state.keyboard.clone().input::<(), _>(
                                    state,
                                    133_u32.into(),
                                    smithay::backend::input::KeyState::Released,
                                    serial,
                                    1,
                                    |_, _, _| smithay::input::keyboard::FilterResult::Forward,
                                );
                            }
                        }
                        Action::Xwayland => {
                            result = server.start_xwayland().unwrap();
                        }
                        Action::Count => {
                            result = state.windows.len() as u32;
                        }
                        Action::Focus(i) => {
                            state.model.focus = Some(state.windows[i].0);
                            state.sync_focus();
                        }
                        Action::IconPresent => {
                            let mut scene = Vec::new();
                            mimshel_compositor::render::scene(state, &mut scene);
                            result = u32::from(
                                matches!(scene.first(), Some(mimshel_compositor::render::SceneElement::Surface { location, lock: false, .. }) if *location == smithay::utils::Point::from((6, 13))),
                            );
                        }
                        Action::Release => {
                            state.pointer_handle.clone().button(
                                state,
                                &smithay::input::pointer::ButtonEvent {
                                    serial,
                                    time: 2,
                                    button: 0x110,
                                    state: smithay::backend::input::ButtonState::Released,
                                },
                            );
                        }
                        Action::DragStart
                        | Action::DragTarget
                        | Action::Press(_)
                        | Action::Target(_) => {
                            let i = match action {
                                Action::DragStart => 0,
                                Action::DragTarget => 1,
                                Action::Press(i) | Action::Target(i) => i,
                                _ => unreachable!(),
                            };
                            let surface =
                                mimshel_compositor::xwayland::surface(&state.windows[i].1).unwrap();
                            let pointer = state.pointer_handle.clone();
                            state.pointer = (10., 10.).into();
                            let focus =
                                mimshel_compositor::xwayland::Focus::pointer(state, surface);
                            pointer.motion(
                                state,
                                Some((focus, (0., 0.).into())),
                                &smithay::input::pointer::MotionEvent {
                                    location: (10., 10.).into(),
                                    serial,
                                    time: 1,
                                },
                            );
                            if matches!(action, Action::DragStart | Action::Press(_)) {
                                pointer.button(
                                    state,
                                    &smithay::input::pointer::ButtonEvent {
                                        serial,
                                        time: 1,
                                        button: 0x110,
                                        state: smithay::backend::input::ButtonState::Pressed,
                                    },
                                );
                            }
                            pointer.frame(state);
                        }
                        Action::Key(code, pressed) => {
                            state.keyboard.clone().input::<(), _>(
                                state,
                                code.into(),
                                if pressed {
                                    smithay::backend::input::KeyState::Pressed
                                } else {
                                    smithay::backend::input::KeyState::Released
                                },
                                serial,
                                1,
                                |_, _, _| smithay::input::keyboard::FilterResult::Forward,
                            );
                        }
                    }
                    reply.send(result).unwrap();
                }
            }
        });
        let result = Self {
            tx,
            ack,
            thread: Some(thread),
        };
        result.ack.recv_timeout(Duration::from_secs(30)).unwrap();
        result
    }
    fn action(&self, a: Action) -> u32 {
        self.tx.send(a).unwrap();
        self.ack.recv_timeout(Duration::from_secs(30)).unwrap()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.tx.send(Action::Stop);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}
#[test]
fn clipboard_primary_drag_and_shared_layout() {
    let runtime = tempfile::tempdir().unwrap();
    let path = runtime.path().join("selection");
    let server = Harness::new(path.clone());
    let mut a = Peer::new(&path);
    let mut b = Peer::new(&path);
    for reverse in [false, true] {
        let (source, target, index) = if reverse {
            (&mut b, &mut a, 1)
        } else {
            (&mut a, &mut b, 0)
        };
        server.action(Action::Focus(index));
        source.sync();
        source.copy();
        server.action(Action::Focus(1 - index));
        target.sync();
        assert!(target.client.mimes.iter().any(|s| s == "text/uri-list"));
        assert!(
            target
                .client
                .primary_mimes
                .iter()
                .any(|s| s == "text/plain")
        );
        for primary in [false, true] {
            let (mut reader, writer) = UnixStream::pair().unwrap();
            reader
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            if primary {
                target
                    .client
                    .primary_offer
                    .as_ref()
                    .unwrap()
                    .receive("text/plain".into(), writer.as_fd());
            } else {
                target
                    .client
                    .offer
                    .as_ref()
                    .unwrap()
                    .receive("text/uri-list".into(), writer.as_fd());
            }
            target.sync();
            source.sync();
            drop(writer);
            let mut text = String::new();
            reader.read_to_string(&mut text).unwrap();
            assert_eq!(
                text,
                if primary {
                    "primary text"
                } else {
                    "file:///tmp/selection-test\r\n"
                }
            );
        }
    }
    server.action(Action::Focus(0));
    a.sync();
    for (code, down) in [(56, true), (42, true), (42, false), (56, false)] {
        server.action(Action::Key(code + 8, down));
    }
    a.sync();
    assert_eq!(a.client.group, 1);
    server.action(Action::Focus(1));
    b.sync();
    assert_eq!(b.client.group, 1);
    server.action(Action::Focus(0));
    a.sync();
    assert_eq!(a.client.group, 1);
    assert_eq!(server.action(Action::Gesture(false)), 0);
    assert_eq!(server.action(Action::Gesture(true)), 0);
    server.action(Action::DragStart);
    a.sync();
    assert_ne!(a.client.serial, 0);
    let source = a
        .client
        .manager
        .as_ref()
        .unwrap()
        .create_data_source(&a.queue.handle(), ());
    source.offer("text/uri-list".into());
    let icon = a
        .client
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&a.queue.handle(), ());
    a.data
        .start_drag(Some(&source), &a.surface, Some(&icon), a.client.serial);
    let icon_file = tempfile::tempfile().unwrap();
    icon_file.set_len(8 * 8 * 4).unwrap();
    let pool = a.client.shm.as_ref().unwrap().create_pool(
        icon_file.as_fd(),
        8 * 8 * 4,
        &a.queue.handle(),
        (),
    );
    let buffer = pool.create_buffer(0, 8, 8, 32, wl_shm::Format::Argb8888, &a.queue.handle(), ());
    icon.attach(Some(&buffer), -4, 3);
    icon.commit();
    a.sync();
    b.client.mimes.clear();
    server.action(Action::DragTarget);
    b.sync();
    assert!(b.client.drag);
    assert!(b.client.mimes.iter().any(|s| s == "text/uri-list"));
    assert_eq!(server.action(Action::IconPresent), 1);
    server.action(Action::Release);
    assert_eq!(server.action(Action::IconPresent), 0);
}

#[test]
fn x11_clipboard_and_primary_both_directions() {
    use x11rb::{
        connection::Connection as _,
        protocol::{
            Event,
            xproto::{
                self, AtomEnum, ConnectionExt, CreateWindowAux, PropMode, SelectionNotifyEvent,
            },
        },
        wrapper::ConnectionExt as _,
    };
    if std::process::Command::new("Xwayland")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("SKIP: Xwayland is missing from PATH");
        return;
    }
    let runtime = tempfile::tempdir().unwrap();
    let path = runtime.path().join("x11-selection");
    let server = Harness::new(path.clone());
    let mut peer = Peer::new(&path);
    let display = server.action(Action::Xwayland);
    let (connection, screen) = x11rb::connect(Some(&format!(":{display}"))).unwrap();
    let window = connection.generate_id().unwrap();
    let root = connection.setup().roots[screen].root;
    connection
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            100,
            80,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap();
    connection.map_window(window).unwrap();
    connection.flush().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while server.action(Action::Count) < 2 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let clipboard = atom(b"CLIPBOARD");
    let primary = u32::from(AtomEnum::PRIMARY);
    let targets = atom(b"TARGETS");
    let text = atom(b"text/plain");
    let legacy_text = atom(b"TEXT");
    let property = atom(b"TEST_SELECTION");
    let uri = atom(b"text/uri-list");
    for selection in [clipboard, primary] {
        server.action(Action::Focus(1));
        connection
            .set_selection_owner(window, selection, x11rb::CURRENT_TIME)
            .unwrap();
        connection.flush().unwrap();
        // Answer the WM's TARGETS query and then its actual data request.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut advertised = false;
        loop {
            while let Some(event) = connection.poll_for_event().unwrap() {
                if let Event::SelectionRequest(request) = event {
                    let prop = if request.property == 0 {
                        request.target
                    } else {
                        request.property
                    };
                    if request.target == targets {
                        connection
                            .change_property32(
                                PropMode::REPLACE,
                                request.requestor,
                                prop,
                                AtomEnum::ATOM,
                                &[targets, text, legacy_text],
                            )
                            .unwrap();
                        advertised = true;
                    } else {
                        assert!(request.target == text || request.target == legacy_text);
                        connection
                            .change_property8(
                                PropMode::REPLACE,
                                request.requestor,
                                prop,
                                text,
                                b"X11 selection bytes",
                            )
                            .unwrap();
                    }
                    connection
                        .send_event(
                            false,
                            request.requestor,
                            xproto::EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: xproto::SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: request.time,
                                requestor: request.requestor,
                                selection: request.selection,
                                target: request.target,
                                property: prop,
                            },
                        )
                        .unwrap();
                    connection.flush().unwrap();
                }
            }
            if advertised {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        server.action(Action::Focus(0));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            peer.sync();
            if if selection == clipboard {
                peer.client.mimes.iter().any(|s| s == "text/plain")
            } else {
                peer.client.primary_mimes.iter().any(|s| s == "text/plain")
            } {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
        }
        let (mut reader, writer) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        if selection == clipboard {
            peer.client
                .offer
                .as_ref()
                .unwrap()
                .receive("text/plain".into(), writer.as_fd());
        } else {
            peer.client
                .primary_offer
                .as_ref()
                .unwrap()
                .receive("text/plain".into(), writer.as_fd());
        }
        peer.sync();
        drop(writer);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(Event::SelectionRequest(request)) = connection.poll_for_event().unwrap() {
                let is_data = request.target != targets;
                if is_data {
                    assert!(request.target == text || request.target == legacy_text);
                    connection
                        .change_property8(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            text,
                            b"X11 selection bytes",
                        )
                        .unwrap();
                } else {
                    connection
                        .change_property32(
                            PropMode::REPLACE,
                            request.requestor,
                            request.property,
                            AtomEnum::ATOM,
                            &[targets, text, legacy_text],
                        )
                        .unwrap();
                }
                connection
                    .send_event(
                        false,
                        request.requestor,
                        xproto::EventMask::NO_EVENT,
                        SelectionNotifyEvent {
                            response_type: xproto::SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: request.time,
                            requestor: request.requestor,
                            selection: request.selection,
                            target: request.target,
                            property: request.property,
                        },
                    )
                    .unwrap();
                connection.flush().unwrap();
                if is_data {
                    break;
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut bytes = String::new();
        reader.read_to_string(&mut bytes).unwrap();
        assert_eq!(bytes, "X11 selection bytes");
    }
    peer.copy();
    server.action(Action::Focus(1));
    for (selection, mime, expected) in [
        (clipboard, uri, "file:///tmp/selection-test\r\n"),
        (primary, text, "primary text"),
    ] {
        connection
            .convert_selection(window, selection, mime, property, x11rb::CURRENT_TIME)
            .unwrap();
        connection.flush().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            peer.sync();
            if let Some(Event::SelectionNotify(event)) = connection.poll_for_event().unwrap() {
                assert_eq!(event.property, property);
                let reply = connection
                    .get_property(true, window, property, AtomEnum::ANY, 0, 4096)
                    .unwrap()
                    .reply()
                    .unwrap();
                assert_eq!(reply.value, expected.as_bytes());
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn xdnd_uri_list_both_directions() {
    use x11rb::{
        connection::Connection as _,
        protocol::{
            Event,
            xproto::{
                self, AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask,
                PropMode, SelectionNotifyEvent,
            },
        },
        wrapper::ConnectionExt as _,
    };
    if std::process::Command::new("Xwayland")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("SKIP: XDND integration needs Xwayland in PATH");
        return;
    }
    let runtime = tempfile::tempdir().unwrap();
    let path = runtime.path().join("xdnd");
    let server = Harness::new(path.clone());
    let mut peer = Peer::new(&path);
    let display = server.action(Action::Xwayland);
    let (x, screen) = x11rb::connect(Some(&format!(":{display}"))).unwrap();
    let root = x.setup().roots[screen].root;
    let window = x.generate_id().unwrap();
    x.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        100,
        80,
        0,
        xproto::WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .unwrap();
    let atom = |name: &[u8]| x.intern_atom(false, name).unwrap().reply().unwrap().atom;
    let aware = atom(b"XdndAware");
    let enter = atom(b"XdndEnter");
    let position = atom(b"XdndPosition");
    let status = atom(b"XdndStatus");
    let drop_atom = atom(b"XdndDrop");
    let finished = atom(b"XdndFinished");
    let selection = atom(b"XdndSelection");
    let copy = atom(b"XdndActionCopy");
    let uri = atom(b"text/uri-list");
    let property = atom(b"MIMSHEL_XDND_TEST");
    x.change_property32(PropMode::REPLACE, window, aware, AtomEnum::ATOM, &[5])
        .unwrap();
    x.map_window(window).unwrap();
    x.flush().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while server.action(Action::Count) < 2 {
        assert!(std::time::Instant::now() < deadline, "X11 map timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    let send = |target, type_, data: [u32; 5]| {
        x.send_event(
            false,
            target,
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, target, type_, data),
        )
        .unwrap();
        x.flush().unwrap();
    };
    // Wayland source -> X11 destination: negotiate Copy, then request the URI bytes.
    server.action(Action::Press(0));
    peer.sync();
    let source = peer
        .client
        .manager
        .as_ref()
        .unwrap()
        .create_data_source(&peer.queue.handle(), ());
    source.offer("text/uri-list".into());
    source.set_actions(wl_data_device_manager::DndAction::Copy);
    peer.data
        .start_drag(Some(&source), &peer.surface, None, peer.client.serial);
    peer.sync();
    server.action(Action::Target(1));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let proxy = loop {
        peer.sync();
        if let Some(Event::ClientMessage(e)) = x.poll_for_event().unwrap() {
            if e.type_ == enter {
                assert!(e.data.as_data32()[2..].contains(&uri));
            }
            if e.type_ == position {
                let proxy = e.data.as_data32()[0];
                send(proxy, status, [window, 1, 0, 0, copy]);
                break proxy;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "XDND position timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    // A roundtrip on X11 ensures Status reaches XWayland before releasing.
    x.get_input_focus().unwrap().reply().unwrap();
    peer.sync();
    server.action(Action::Release);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(Event::ClientMessage(e)) = x.poll_for_event().unwrap()
            && e.type_ == drop_atom
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "XDND drop timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    x.convert_selection(window, selection, uri, property, x11rb::CURRENT_TIME)
        .unwrap();
    x.flush().unwrap();
    loop {
        peer.sync();
        if let Some(Event::SelectionNotify(e)) = x.poll_for_event().unwrap() {
            assert_eq!(e.property, property);
            let bytes = x
                .get_property(true, window, property, AtomEnum::ANY, 0, 4096)
                .unwrap()
                .reply()
                .unwrap()
                .value;
            assert_eq!(bytes, b"file:///tmp/selection-test\r\n");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Wayland -> X11 URI transfer timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    send(proxy, finished, [window, 1, copy, 0, 0]);
    peer.sync();
    // X11 source -> Wayland destination: X11Wm creates a proxy over the X root.
    server.action(Action::Press(1));
    x.set_selection_owner(window, selection, x11rb::CURRENT_TIME)
        .unwrap();
    x.flush().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let proxy = loop {
        let children = x.query_tree(root).unwrap().reply().unwrap().children;
        let found = children.into_iter().find(|child| {
            x.get_property(false, *child, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 128)
                .unwrap()
                .reply()
                .is_ok_and(|p| p.value == b"Smithay XDND proxy")
        });
        if let Some(proxy) = found {
            break proxy;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "incoming XDND proxy timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    send(proxy, enter, [window, 5 << 24, uri, 0, 0]);
    send(proxy, position, [window, 0, 0, x11rb::CURRENT_TIME, copy]);
    x.get_input_focus().unwrap().reply().unwrap();
    peer.client.mimes.clear();
    server.action(Action::Target(0));
    loop {
        peer.sync();
        if peer.client.mimes.iter().any(|m| m == "text/uri-list") {
            break;
        }
        server.action(Action::Target(0));
        assert!(
            std::time::Instant::now() < deadline,
            "incoming URI offer timed out"
        );
    }
    let offer = peer.client.offer.as_ref().unwrap();
    offer.accept(peer.client.serial, Some("text/uri-list".into()));
    offer.set_actions(
        wl_data_device_manager::DndAction::Copy,
        wl_data_device_manager::DndAction::Copy,
    );
    peer.sync();
    send(proxy, drop_atom, [window, 0, x11rb::CURRENT_TIME, 0, 0]);
    server.action(Action::Release);
    let (mut reader, writer) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    peer.client
        .offer
        .as_ref()
        .unwrap()
        .receive("text/uri-list".into(), writer.as_fd());
    peer.sync();
    drop(writer);
    loop {
        if let Some(Event::SelectionRequest(r)) = x.poll_for_event().unwrap() {
            assert_eq!(r.target, uri);
            x.change_property8(
                PropMode::REPLACE,
                r.requestor,
                r.property,
                uri,
                b"file:///tmp/from-x11\r\n",
            )
            .unwrap();
            x.send_event(
                false,
                r.requestor,
                EventMask::NO_EVENT,
                SelectionNotifyEvent {
                    response_type: xproto::SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: r.time,
                    requestor: r.requestor,
                    selection: r.selection,
                    target: r.target,
                    property: r.property,
                },
            )
            .unwrap();
            x.flush().unwrap();
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "X11 URI request timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut bytes = String::new();
    reader.read_to_string(&mut bytes).unwrap();
    assert_eq!(bytes, "file:///tmp/from-x11\r\n");
    peer.client.offer.as_ref().unwrap().finish();
    peer.sync();
    loop {
        if let Some(Event::ClientMessage(e)) = x.poll_for_event().unwrap()
            && e.type_ == finished
        {
            assert_eq!(e.data.as_data32()[1], 1);
            assert_eq!(e.data.as_data32()[2], copy);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "X11 source did not receive XdndFinished"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
