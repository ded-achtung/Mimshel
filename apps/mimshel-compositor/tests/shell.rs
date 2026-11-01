#[path = "support/input.rs"]
mod input;
use mimshel_compositor::backend::headless::Server;
use std::{
    io::Write,
    os::{fd::AsFd, unix::net::UnixStream},
    sync::mpsc,
    time::Duration,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
#[derive(Default)]
struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    shell: Option<xdg_wm_base::XdgWmBase>,
    decoration: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    mode: Option<zxdg_toplevel_decoration_v1::Mode>,
    size: (i32, i32),
    output: Option<wl_output::WlOutput>,
    seat: Option<wl_seat::WlSeat>,
    layer: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    foreign: Option<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    list: Option<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1>,
    handles: Vec<(
        zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        String,
    )>,
    ext_titles: Vec<String>,
    lock: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
    locked: usize,
    finished: usize,
    idle: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    idled: usize,
    resumed: usize,
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    token: Option<String>,
    app_ids: Vec<String>,
    close_requests: usize,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_output" => state.output = Some(registry.bind(name, 3, qh, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, 7, qh, ())),
                "zwlr_layer_shell_v1" => state.layer = Some(registry.bind(name, 4, qh, ())),
                "zwlr_foreign_toplevel_manager_v1" => {
                    state.foreign = Some(registry.bind(name, 3, qh, ()))
                }
                "ext_foreign_toplevel_list_v1" => state.list = Some(registry.bind(name, 1, qh, ())),
                "ext_session_lock_manager_v1" => state.lock = Some(registry.bind(name, 1, qh, ())),
                "ext_idle_notifier_v1" => state.idle = Some(registry.bind(name, 1, qh, ())),
                "xdg_activation_v1" => state.activation = Some(registry.bind(name, 1, qh, ())),
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "zxdg_decoration_manager_v1" => {
                    state.decoration = Some(registry.bind(name, 1, qh, ()))
                }
                "xdg_wm_base" => state.shell = Some(registry.bind(name, version.min(3), qh, ())),
                _ => {}
            }
        }
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        shell: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        _: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
        }
    }
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore wl_buffer::WlBuffer);
delegate_noop!(Client: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => state.size = (width, height),
            xdg_toplevel::Event::Close => state.close_requests += 1,
            _ => {}
        }
    }
}
impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: wayland_client::WEnum::Value(mode),
        } = event
        {
            state.mode = Some(mode);
        }
    }
}

use wayland_protocols::{
    ext::{
        foreign_toplevel_list::v1::client::{
            ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
        },
        idle_notify::v1::client::{ext_idle_notification_v1, ext_idle_notifier_v1},
        session_lock::v1::client::{
            ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
        },
    },
    xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1},
};
use wayland_protocols_wlr::{
    foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
    },
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
};
delegate_noop!(Client: ignore wl_output::WlOutput);
delegate_noop!(Client: ignore wl_seat::WlSeat);
delegate_noop!(Client: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(Client: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1);
delegate_noop!(Client: ignore ext_idle_notifier_v1::ExtIdleNotifierV1);
delegate_noop!(Client: ignore xdg_activation_v1::XdgActivationV1);
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Client {
    fn event(
        _: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            surface.ack_configure(serial);
        }
    }
}
impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.handles.push((toplevel, String::new()));
        }
    }
    wayland_client::event_created_child!(Client, zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, [0 => (zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ())]);
}
impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for Client {
    fn event(
        state: &mut Self,
        h: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_handle_v1::Event::Title { title } = event
            && let Some((_, t)) = state.handles.iter_mut().find(|(handle, _)| handle == h)
        {
            *t = title;
        }
    }
}
impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for Client {
    fn event(
        _: &mut Self,
        _: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        _: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
    wayland_client::event_created_child!(Client, ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, [0 => (ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ())]);
}
impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_handle_v1::Event::Title { title } => state.ext_titles.push(title),
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => state.app_ids.push(app_id),
            _ => {}
        }
    }
}
impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => state.locked += 1,
            ext_session_lock_v1::Event::Finished => state.finished += 1,
            _ => {}
        }
    }
}
impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, ()> for Client {
    fn event(
        _: &mut Self,
        surface: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, .. } = event {
            surface.ack_configure(serial);
        }
    }
}
impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.idled += 1,
            ext_idle_notification_v1::Event::Resumed => state.resumed += 1,
            _ => {}
        }
    }
}
impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.token = Some(token);
        }
    }
}

type Command = Box<dyn FnOnce(&mut mimshel_compositor::state::State) + Send>;
struct Harness {
    runtime: tempfile::TempDir,
    commands: mpsc::Sender<Option<Command>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Harness {
    fn new() -> Self {
        let runtime = tempfile::tempdir().unwrap();
        let path = runtime.path().join("shell");
        let (commands, requests) = mpsc::channel::<Option<Command>>();
        let (ready, started) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut server = Server::private(&path).unwrap();
            ready.send(()).unwrap();
            let mut scene = Vec::new();
            loop {
                server.dispatch(Some(Duration::from_millis(2))).unwrap();
                // The headless backend models presentation by inspecting the exact
                // source list consumed by GLES before acknowledging the lock.
                mimshel_compositor::render::scene(&server.state, &mut scene);
                if server.state.protocols.locked {
                    assert!(scene.iter().all(|e| matches!(
                        e,
                        mimshel_compositor::render::SceneElement::Surface { lock: true, .. }
                    )));
                }
                server.state.lock_frame_presented();
                match requests.try_recv() {
                    Ok(Some(command)) => command(&mut server.state),
                    Ok(None) => break,
                    Err(_) => {}
                }
            }
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            runtime,
            commands,
            thread: Some(thread),
        }
    }
    fn client(&self) -> Peer {
        Peer::new(&self.runtime.path().join("shell"))
    }
    fn inspect<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut mimshel_compositor::state::State) -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = mpsc::channel();
        self.commands
            .send(Some(Box::new(move |s| tx.send(f(s)).unwrap())))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.commands.send(None);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}
struct Peer {
    _connection: Connection,
    queue: wayland_client::EventQueue<Client>,
    state: Client,
}
impl Peer {
    fn new(path: &std::path::Path) -> Self {
        let connection = Connection::from_socket(UnixStream::connect(path).unwrap()).unwrap();
        let mut queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        let mut state = Client::default();
        queue.roundtrip(&mut state).unwrap();
        queue.roundtrip(&mut state).unwrap();
        Self {
            _connection: connection,
            queue,
            state,
        }
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
        self.queue.roundtrip(&mut self.state).unwrap();
    }
    fn buffer(&self, surface: &wl_surface::WlSurface, w: i32, h: i32) {
        let qh = self.queue.handle();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![255; (w * h * 4) as usize]).unwrap();
        let pool = self
            .state
            .shm
            .as_ref()
            .unwrap()
            .create_pool(file.as_fd(), w * h * 4, &qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
    }
    fn window(&mut self, title: &str) -> (wl_surface::WlSurface, xdg_toplevel::XdgToplevel) {
        let qh = self.queue.handle();
        let surface = self
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let xdg = self
            .state
            .shell
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_title(title.into());
        top.set_app_id(format!("test.{title}"));
        surface.commit();
        self.sync();
        self.buffer(&surface, 100, 80);
        self.sync();
        (surface, top)
    }
}

#[test]
fn exclusive_layer_constrains_placement_and_maximize() {
    let h = Harness::new();
    let mut p = h.client();
    let qh = p.queue.handle();
    let surface = p.state.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = p.state.layer.as_ref().unwrap().get_layer_surface(
        &surface,
        p.state.output.as_ref(),
        zwlr_layer_shell_v1::Layer::Top,
        "panel".into(),
        &qh,
        (),
    );
    layer.set_size(0, 32);
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_margin(5, 0, 0, 0);
    layer.set_exclusive_zone(32);
    surface.commit();
    p.sync();
    p.buffer(&surface, 1280, 32);
    p.sync();
    let (_, top) = p.window("one");
    assert!(h.inspect(|s| s.model.windows[0].rect.y) >= 37);
    top.set_maximized();
    p.sync();
    assert_eq!(p.state.size, (1280, 733));
    assert_eq!(h.inspect(|s| s.model.windows[0].rect.y), 37);
    top.set_fullscreen(None);
    p.sync();
    assert_eq!(p.state.size, (1280, 800));
    layer.destroy();
    surface.destroy();
    p.sync();
    assert_eq!(h.inspect(|s| s.model.area.y), 0);
}

#[test]
fn foreign_lists_and_activation_raise_bottom_window() {
    let h = Harness::new();
    let mut windows = h.client();
    let mut dock = h.client();
    let (a, _) = windows.window("one");
    let _b = windows.window("two");
    dock.sync();
    assert_eq!(dock.state.handles.len(), 2);
    assert_eq!(dock.state.ext_titles, ["one", "two"]);
    assert_eq!(dock.state.app_ids, ["test.one", "test.two"]);
    let before = h.inspect(|s| s.model.focus);
    dock.state
        .handles
        .iter()
        .find(|(_, t)| t == "one")
        .unwrap()
        .0
        .activate(dock.state.seat.as_ref().unwrap());
    dock.sync();
    assert_ne!(h.inspect(|s| s.model.focus), before);
    assert_eq!(
        h.inspect(|s| s.model.focus),
        h.inspect(|s| s.model.windows.last().map(|w| w.id))
    );
    dock.state
        .handles
        .iter()
        .find(|(_, title)| title == "two")
        .unwrap()
        .0
        .activate(dock.state.seat.as_ref().unwrap());
    dock.sync();
    assert_eq!(h.inspect(|s| s.model.focus), before);
    let token = dock
        .state
        .activation
        .as_ref()
        .unwrap()
        .get_activation_token(&dock.queue.handle(), ());
    token.commit();
    dock.sync();
    let token_text = dock.state.token.take().unwrap();
    windows
        .state
        .activation
        .as_ref()
        .unwrap()
        .activate(token_text.clone(), &a);
    windows.sync();
    let first = h.inspect(|s| s.model.focus);
    assert_ne!(first, before);
    dock.state
        .handles
        .iter()
        .find(|(_, t)| t == "two")
        .unwrap()
        .0
        .activate(dock.state.seat.as_ref().unwrap());
    dock.sync();
    assert_ne!(h.inspect(|s| s.model.focus), first);
    windows
        .state
        .activation
        .as_ref()
        .unwrap()
        .activate(token_text, &a);
    windows.sync();
    assert_ne!(h.inspect(|s| s.model.focus), first);
    // Consumed tokens cannot raise a second time.
    let handle = &dock
        .state
        .handles
        .iter()
        .find(|(_, title)| title == "two")
        .unwrap()
        .0;
    handle.set_maximized();
    dock.sync();
    assert_eq!(
        h.inspect(|s| s.model.windows.last().unwrap().mode),
        mimshel_compositor::model::Mode::Maximized
    );
    let handle = &dock
        .state
        .handles
        .iter()
        .find(|(_, title)| title == "two")
        .unwrap()
        .0;
    handle.set_minimized();
    dock.sync();
    assert_eq!(h.inspect(|s| s.space.elements().count()), 1);
    let handle = &dock
        .state
        .handles
        .iter()
        .find(|(_, title)| title == "two")
        .unwrap()
        .0;
    handle.unset_minimized();
    dock.sync();
    assert_eq!(
        h.inspect(|s| s.model.windows.last().unwrap().mode),
        mimshel_compositor::model::Mode::Maximized
    );
    let handle = &dock
        .state
        .handles
        .iter()
        .find(|(_, title)| title == "two")
        .unwrap()
        .0;
    handle.unset_maximized();
    handle.close();
    dock.sync();
    windows.sync();
    assert_eq!(windows.state.close_requests, 1);
    token.destroy();
}

#[test]
fn lock_client_death_keeps_scene_black_until_replacement_unlocks() {
    let h = Harness::new();
    let mut app = h.client();
    let _window = app.window("private");
    {
        let mut lock = h.client();
        let qh = lock.queue.handle();
        let session = lock.state.lock.as_ref().unwrap().lock(&qh, ());
        lock.sync();
        assert_eq!(lock.state.locked, 1);
        let surface = lock
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let _ls = session.get_lock_surface(&surface, lock.state.output.as_ref().unwrap(), &qh, ());
        lock.sync();
        lock.buffer(&surface, 1280, 800);
        lock.sync();
        assert_eq!(
            h.inspect(|s| {
                let mut scene = Vec::new();
                mimshel_compositor::render::scene(s, &mut scene);
                scene.len()
            }),
            1
        );
        let mut rival = h.client();
        let _rejected = rival
            .state
            .lock
            .as_ref()
            .unwrap()
            .lock(&rival.queue.handle(), ());
        rival.sync();
        assert_eq!(rival.state.finished, 1);
        assert!(h.inspect(|s| s.protocols.locked));
        // Drop the connection and all proxies, as when a locker is killed.
    }
    app.sync();
    assert!(h.inspect(|s| s.protocols.locked));
    assert!(h.inspect(|s| {
        let mut scene = Vec::new();
        mimshel_compositor::render::scene(s, &mut scene);
        scene.is_empty() && s.keyboard.current_focus().is_none() && s.surface_under().is_none()
    }));
    let mut replacement = h.client();
    let qh = replacement.queue.handle();
    let session = replacement.state.lock.as_ref().unwrap().lock(&qh, ());
    replacement.sync();
    assert_eq!(replacement.state.locked, 1);
    let surface = replacement
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&qh, ());
    let _ls = session.get_lock_surface(
        &surface,
        replacement.state.output.as_ref().unwrap(),
        &qh,
        (),
    );
    replacement.sync();
    replacement.buffer(&surface, 1280, 800);
    replacement.sync();
    session.unlock_and_destroy();
    replacement.sync();
    assert!(!h.inspect(|s| s.protocols.locked));
}

#[test]
fn idle_timeout_and_input_reset_do_not_request_frames() {
    let h = Harness::new();
    let mut p = h.client();
    let _notification = p.state.idle.as_ref().unwrap().get_idle_notification(
        60,
        p.state.seat.as_ref().unwrap(),
        &p.queue.handle(),
        (),
    );
    p.sync();
    h.inspect(|s| s.dirty = false);
    std::thread::sleep(Duration::from_millis(90));
    p.sync();
    assert_eq!(p.state.idled, 1);
    assert!(!h.inspect(|s| s.dirty));
    h.inspect(input::click);
    p.sync();
    assert_eq!(p.state.resumed, 1);
    std::thread::sleep(Duration::from_millis(90));
    p.sync();
    assert_eq!(p.state.idled, 2);
}

#[test]
fn layer_keyboard_interactivity_and_unmap() {
    let h = Harness::new();
    let mut app = h.client();
    let _window = app.window("app");
    let mut panel = h.client();
    let qh = panel.queue.handle();
    let surface = panel
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&qh, ());
    let layer = panel.state.layer.as_ref().unwrap().get_layer_surface(
        &surface,
        panel.state.output.as_ref(),
        zwlr_layer_shell_v1::Layer::Overlay,
        "panel".into(),
        &qh,
        (),
    );
    layer.set_size(0, 32);
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    surface.commit();
    panel.sync();
    panel.buffer(&surface, 1280, 32);
    panel.sync();
    let app_focus = h.inspect(|s| s.keyboard.current_focus().map(|f| f.id()));
    h.inspect(|s| {
        s.pointer = (100.0, 10.0).into();
        input::click(s);
    });
    assert_eq!(
        h.inspect(|s| s.keyboard.current_focus().map(|f| f.id())),
        app_focus
    );
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand);
    surface.commit();
    panel.sync();
    h.inspect(input::click);
    assert_ne!(
        h.inspect(|s| s.keyboard.current_focus().map(|f| f.id())),
        app_focus
    );
    let focused_panel = h.inspect(|s| s.keyboard.current_focus().map(|f| f.id()));
    surface.commit();
    panel.sync();
    assert_eq!(
        h.inspect(|s| s.keyboard.current_focus().map(|f| f.id())),
        focused_panel
    );
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    surface.commit();
    panel.sync();
    h.inspect(|s| {
        s.pointer = (100.0, 100.0).into();
        input::click(s);
    });
    assert_ne!(
        h.inspect(|s| s.keyboard.current_focus().map(|f| f.id())),
        app_focus
    );
    surface.attach(None, 0, 0);
    surface.commit();
    panel.sync();
    assert_eq!(
        h.inspect(|s| s.keyboard.current_focus().map(|f| f.id())),
        app_focus
    );
    assert!(h.inspect(|s| {
        let mut scene = Vec::new();
        mimshel_compositor::render::scene(s, &mut scene);
        scene
            .iter()
            .all(|e| matches!(e, mimshel_compositor::render::SceneElement::Window(_)))
    }));
}

#[test]
fn layer_render_and_pointer_order_follow_protocol_layers() {
    let h = Harness::new();
    let mut p = h.client();
    let _window = p.window("window");
    let qh = p.queue.handle();
    let mut surfaces = Vec::new();
    // Creation order deliberately differs from protocol stacking order.
    for (kind, namespace) in [
        (zwlr_layer_shell_v1::Layer::Overlay, "overlay"),
        (zwlr_layer_shell_v1::Layer::Background, "background"),
        (zwlr_layer_shell_v1::Layer::Top, "top"),
        (zwlr_layer_shell_v1::Layer::Bottom, "bottom"),
    ] {
        let surface = p.state.compositor.as_ref().unwrap().create_surface(&qh, ());
        let layer = p.state.layer.as_ref().unwrap().get_layer_surface(
            &surface,
            p.state.output.as_ref(),
            kind,
            namespace.into(),
            &qh,
            (),
        );
        layer.set_size(0, 32);
        layer.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        surface.commit();
        p.sync();
        p.buffer(&surface, 1280, 32);
        p.sync();
        surfaces.push((surface, layer));
    }
    assert_eq!(
        h.inspect(|s| {
            let mut scene = Vec::new();
            mimshel_compositor::render::scene(s, &mut scene);
            let output = s.space.outputs().next().unwrap();
            let map = smithay::desktop::layer_map_for_output(output);
            scene
                .iter()
                .map(|e| match e {
                    mimshel_compositor::render::SceneElement::Window(_) => "window".to_owned(),
                    mimshel_compositor::render::SceneElement::Surface { surface, .. } => map
                        .layer_for_surface(surface, smithay::desktop::WindowSurfaceType::ALL)
                        .unwrap()
                        .namespace()
                        .to_owned(),
                })
                .collect::<Vec<_>>()
        }),
        ["overlay", "top", "window", "bottom", "background"]
    );
    assert_eq!(
        h.inspect(|s| {
            s.pointer = (20.0, 10.0).into();
            let (surface, _) = s.surface_under().unwrap();
            let output = s.space.outputs().next().unwrap();
            smithay::desktop::layer_map_for_output(output)
                .layer_for_surface(&surface, smithay::desktop::WindowSurfaceType::ALL)
                .unwrap()
                .namespace()
                .to_owned()
        }),
        "overlay"
    );
}
