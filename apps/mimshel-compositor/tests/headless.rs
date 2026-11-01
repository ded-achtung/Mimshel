use mimshel_compositor::backend::headless::Server;
use std::{
    io::Write,
    os::{fd::AsFd, unix::net::UnixStream},
    sync::mpsc,
    time::Duration,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
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
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            state.size = (width, height);
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

#[test]
fn decoration_and_premap_maximize() {
    use zxdg_toplevel_decoration_v1::Mode;
    let runtime = tempfile::tempdir().unwrap();
    let path = runtime.path().join("decoration-test");
    let (stop, requests) = mpsc::channel();
    let (snapshots, responses) = mpsc::channel();
    let (ready, started) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut server = Server::private(&path).unwrap();
        ready.send(()).unwrap();
        loop {
            server.dispatch(Some(Duration::from_millis(5))).unwrap();
            match requests.try_recv() {
                Ok(true) => {
                    let window = server.state.model.windows.first().unwrap();
                    snapshots
                        .send((
                            window.rect,
                            window.mode,
                            server.state.model.focus,
                            server.state.space.elements().count(),
                        ))
                        .unwrap();
                }
                Ok(false) => break,
                Err(_) => {}
            }
        }
    });
    started.recv_timeout(Duration::from_secs(30)).unwrap();
    let connection = Connection::from_socket(
        UnixStream::connect(runtime.path().join("decoration-test")).unwrap(),
    )
    .unwrap();
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut client = Client::default();
    queue.roundtrip(&mut client).unwrap();
    let surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
    let xdg = client
        .shell
        .as_ref()
        .unwrap()
        .get_xdg_surface(&surface, &qh, ());
    let top = xdg.get_toplevel(&qh, ());
    let decoration = client
        .decoration
        .as_ref()
        .unwrap()
        .get_toplevel_decoration(&top, &qh, ());
    top.set_maximized();
    surface.commit();
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.mode, Some(Mode::ServerSide));
    assert_eq!(client.size, (1280, 770));
    top.set_fullscreen(None);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.size, (1280, 800));
    top.unset_fullscreen();
    top.set_maximized();
    decoration.set_mode(Mode::ClientSide);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.mode, Some(Mode::ClientSide));
    assert_eq!(client.size, (1280, 800));
    decoration.unset_mode();
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.mode, Some(Mode::ServerSide));
    assert_eq!(client.size, (1280, 770));
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&vec![255; 100 * 80 * 4]).unwrap();
    let pool = client
        .shm
        .as_ref()
        .unwrap()
        .create_pool(file.as_fd(), 100 * 80 * 4, &qh, ());
    let buffer = pool.create_buffer(0, 100, 80, 400, wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    queue.roundtrip(&mut client).unwrap();
    stop.send(true).unwrap();
    let (rect, mode, focus, visible) = responses.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!((rect.width, rect.height), (1280, 770));
    assert_eq!(mode, mimshel_compositor::model::Mode::Maximized);
    assert!(focus.is_some());
    assert_eq!(visible, 1);
    top.set_minimized();
    queue.roundtrip(&mut client).unwrap();
    stop.send(true).unwrap();
    let (_, mode, focus, visible) = responses.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(mode, mimshel_compositor::model::Mode::Minimized);
    assert_eq!(focus, None);
    assert_eq!(visible, 0);
    stop.send(false).unwrap();
    thread.join().unwrap();
}

#[test]
fn real_clients_stack_and_restore_focus() {
    // An absolute socket keeps this test independent of the process environment.
    let runtime = tempfile::tempdir().unwrap();
    let path = runtime.path().join("wayland-test");
    let (commands, requests) = mpsc::channel();
    let (snapshots, responses) = mpsc::channel();
    let (ready, started) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut server = Server::private(&path).unwrap();
        ready.send(()).unwrap();
        loop {
            server.dispatch(Some(Duration::from_millis(10))).unwrap();
            match requests.try_recv() {
                Ok(true) => {
                    snapshots
                        .send((
                            server
                                .state
                                .model
                                .windows
                                .iter()
                                .map(|w| w.id)
                                .collect::<Vec<_>>(),
                            server.state.model.focus,
                        ))
                        .unwrap();
                }
                Ok(false) => break,
                Err(_) => {}
            }
        }
    });
    started.recv_timeout(Duration::from_secs(30)).unwrap();
    let connection =
        Connection::from_socket(UnixStream::connect(runtime.path().join("wayland-test")).unwrap())
            .unwrap();
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut client = Client::default();
    queue.roundtrip(&mut client).unwrap();
    let mut surfaces = Vec::new();
    let mut files = Vec::new();
    for _ in 0..2 {
        let surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = client
            .shell
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![255; 100 * 80 * 4]).unwrap();
        let pool = client
            .shm
            .as_ref()
            .unwrap()
            .create_pool(file.as_fd(), 100 * 80 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 100, 80, 400, wl_shm::Format::Argb8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 100, 80);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        files.push(file);
        surfaces.push((surface, xdg, top, buffer, pool));
    }
    commands.send(true).unwrap();
    let (stack, focus) = responses.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(stack.len(), 2);
    assert_eq!(focus, stack.last().copied());
    let (surface, xdg, top, buffer, pool) = surfaces.pop().unwrap();
    top.destroy();
    xdg.destroy();
    surface.destroy();
    buffer.destroy();
    pool.destroy();
    queue.roundtrip(&mut client).unwrap();
    commands.send(true).unwrap();
    let (remaining, focus) = responses.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(remaining, vec![stack[0]]);
    assert_eq!(focus, Some(stack[0]));
    commands.send(false).unwrap();
    thread.join().unwrap();
}

/// A real shm client for the manual nested smoke test. The harness launches this
/// test as the compositor's command, so it also checks DISPLAY is removed.
#[test]
#[ignore = "launched by the nested smoke driver under Xvfb"]
fn nested_client() {
    assert!(std::env::var_os("DISPLAY").is_none());
    let connection = Connection::connect_to_env().unwrap();
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut client = Client::default();
    queue.roundtrip(&mut client).unwrap();
    let mut windows = Vec::new();
    for color in [[0x40, 0x70, 0xe0, 0xff], [0xd0, 0x90, 0x30, 0xff]] {
        let surface = client.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = client
            .shell
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_title("Mimshel shm smoke".into());
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&color.repeat(300 * 180)).unwrap();
        let pool = client
            .shm
            .as_ref()
            .unwrap()
            .create_pool(file.as_fd(), 300 * 180 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 300, 180, 1200, wl_shm::Format::Argb8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 300, 180);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        windows.push((surface, xdg, top, file, pool, buffer));
    }
    // No frame requests or redraw loop: these surfaces must become truly idle.
    std::thread::sleep(Duration::from_secs(30));
}
