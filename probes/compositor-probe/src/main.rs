#![forbid(unsafe_code)]

mod graphics;
mod input;
mod math;
mod options;
mod state;
mod timing;

use smithay::reexports::rustix::process::{Pid, Signal, kill_process_group};
use smithay::reexports::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use smithay::{
    backend::{
        egl::context::GlAttributes,
        renderer::{ImportDma, ImportEgl, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        wayland_server::{Display, ListeningSocket},
        winit::{dpi::PhysicalSize, platform::pump_events::PumpStatus, window::Window},
    },
    utils::Transform,
};
use state::{ClientState, State};
use std::os::unix::process::CommandExt;
use std::{
    process::{Child, Command},
    sync::Arc,
    time::{Duration, Instant},
};
use x11rb::protocol::xproto::ConnectionExt;

/// Reap completed launchers; terminate remaining launchers when the probe ends.
#[derive(Default)]
struct Children(Vec<Child>);
impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
            }
            let _ = child.wait();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = options::Options::parse()?;
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err("unset WAYLAND_DISPLAY so winit uses X11".into());
    }
    let vblank_mode = std::env::var_os("vblank_mode");
    let unthrottled = vblank_mode.as_deref() == Some(std::ffi::OsStr::new("0"));
    if opts.unthrottled && !unthrottled {
        return Err("--unthrottled requires vblank_mode=0; launch with vblank_mode=0 ./target/release/compositor-probe --unthrottled".into());
    }
    let pacing = match &vblank_mode {
        None => "vsync(driver-default)".to_owned(),
        Some(value) if unthrottled => {
            format!("unthrottled(vblank_mode={})", value.to_string_lossy())
        }
        Some(value) => format!("driver-configured(vblank_mode={})", value.to_string_lossy()),
    };
    if std::env::var_os("DISPLAY").is_none() {
        return Err("an X11 DISPLAY is required (winit backend)".into());
    }
    let (mut backend, mut events) = winit::init_from_attributes_with_gl_attr::<GlesRenderer>(
        Window::default_attributes()
            .with_title("Mimshel compositor probe")
            .with_inner_size(PhysicalSize::new(opts.size.0 as u32, opts.size.1 as u32)),
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: false,
            vsync: !unthrottled,
        },
    )?;
    // Smithay 0.7's winit adapter discards CursorLeft. Query the host pointer
    // outside render timing so dock exit animations do not stick at the edge.
    let (host, _) = x11rb::connect(None)?;
    let host_window = match backend.window().window_handle()?.as_raw() {
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
        _ => return Err("expected winit's Xlib window".into()),
    };
    let mut display = Display::<State>::new()?;
    let mut dh = display.handle();
    // wl_drm/EGL buffers for GPU clients; wl_shm remains available as fallback.
    if let Err(error) = backend.renderer().bind_wl_display(&dh) {
        eprintln!("EGL Wayland binding unavailable; dmabuf/wl_shm remain available: {error}");
    }
    let mut state = State::new(dh.clone())?;
    let _dmabuf_global = state
        .dmabuf
        .create_global::<State>(&dh, backend.renderer().dmabuf_formats());
    let output = Output::new(
        "probe-0".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Mimshel".into(),
            model: "winit probe".into(),
        },
    );
    let _global = output.create_global::<State>(&dh);
    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));
    let socket = ListeningSocket::bind_auto("wayland-probe", 0..32)?;
    let socket_name = socket.socket_name().ok_or("missing socket name")?;
    println!("WAYLAND_DISPLAY={}", socket_name.to_string_lossy());
    println!(
        "backend=winit/X11 renderer=GLES size={}x{} pacing={pacing} wallpaper={} dock={}",
        mode.size.w, mode.size.h, opts.wallpaper, opts.dock,
    );
    let mut graphics = graphics::Graphics::new(backend.renderer(), opts.wallpaper, opts.dock)?;
    let mut children = Children::default();
    for cmd in &opts.spawn {
        let child = Command::new("sh")
            .args(["-c", cmd])
            .process_group(0)
            .env("WAYLAND_DISPLAY", socket_name)
            .env("GDK_BACKEND", "wayland")
            .env_remove("DISPLAY")
            .spawn()?;
        children.0.push(child);
    }
    let start = Instant::now();
    let mut previous = start;
    let mut timing = timing::Timing::new(opts.bench.is_some());
    let mut running = true;
    while running {
        let status = events.dispatch_new_events(|event| match event {
            WinitEvent::CloseRequested => running = false,
            WinitEvent::Input(event) if opts.bench.is_none() => state.input(
                event,
                (backend.window_size().w, backend.window_size().h).into(),
                opts.dock,
            ),
            WinitEvent::Resized { size, .. } if size.w > 0 && size.h > 0 => {
                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
            }
            WinitEvent::Focus(false) => {
                state.pointer_inside = false;
                state.drag = None;
            }
            _ => {}
        });
        if matches!(status, PumpStatus::Exit(_)) || !running {
            break;
        }
        while let Some(stream) = socket.accept()? {
            dh.insert_client(stream, Arc::new(ClientState::default()))?;
        }
        display.dispatch_clients(&mut state)?;
        for (buffer, notifier) in state.imports.drain(..) {
            if backend.renderer().import_dmabuf(&buffer, None).is_ok() {
                let _ = notifier.successful::<State>();
            } else {
                notifier.failed();
            }
        }
        state.space.refresh();
        state.popups.cleanup();
        if opts.bench.is_some_and(|limit| start.elapsed() >= limit) {
            break;
        }
        let size = backend.window_size();
        if size.w == 0 || size.h == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if opts.bench.is_none() {
            let position = host.query_pointer(host_window)?.reply()?;
            state.pointer_inside = position.same_screen
                && position.win_x >= 0
                && position.win_y >= 0
                && i32::from(position.win_x) < size.w
                && i32::from(position.win_y) < size.h;
        }
        let frame_start = Instant::now();
        let dt = frame_start.duration_since(previous).as_secs_f64();
        previous = frame_start;
        let measurements = graphics.render(
            &mut backend,
            &state,
            start.elapsed().as_secs_f64(),
            dt,
            opts.bench.is_some(),
            frame_start,
        )?;
        timing.record(frame_start, measurements);
        for window in state.space.elements() {
            window.send_frame(&output, start.elapsed(), Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        display.flush_clients()?;
        children
            .0
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
    }
    if opts.bench.is_some() {
        let size = backend.window_size();
        println!(
            "{}",
            timing.summary((size.w, size.h), &pacing, opts.wallpaper, opts.dock)
        );
    }
    Ok(())
}
