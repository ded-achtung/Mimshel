//! Nested X11 host window, event loop, and launched client lifecycle.

use crate::{Error, backend::headless::Server, render};
use smithay::reexports::{
    calloop::{Interest, Mode as PollMode, PostAction, generic::Generic},
    rustix::process::{Pid, PidfdFlags, Signal, kill_process_group, pidfd_open},
};
use smithay::{
    backend::{
        egl::context::GlAttributes,
        renderer::{ImportDma, ImportEgl, damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    output::Mode,
    reexports::winit::{dpi::PhysicalSize, window::Window},
    utils::Transform,
};
use std::{
    cell::RefCell,
    ffi::OsString,
    os::unix::process::CommandExt,
    process::{Child, Command},
    rc::Rc,
    time::Instant,
};

/// Own the initial process group even after its leader exits.
struct Launcher {
    child: Rc<RefCell<Option<Child>>>,
    group: Option<Pid>,
}

impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(group) = self.group {
            let _ = kill_process_group(group, Signal::TERM);
        }
        if let Some(mut child) = self.child.borrow_mut().take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

/// Run the nested compositor and manage its launched client.
pub fn run(shell: Vec<OsString>, command: Vec<OsString>) -> Result<(), Error> {
    let mut render_state = render::RenderState::default();
    let mut server = Server::new()?;
    let display = server.start_xwayland()?;
    println!(
        "WAYLAND_DISPLAY={} DISPLAY=:{display}",
        server.socket_name.to_string_lossy()
    );
    let shell_child = if let Some(program) = shell.first() {
        let (server_socket, shell_socket) = std::os::unix::net::UnixStream::pair()?;
        server.state.dh.insert_client(
            server_socket,
            std::sync::Arc::new(crate::state::ClientState::privileged()),
        )?;
        // Stdio installs the socket as fd 0 safely in the child. wayland-client
        // consumes WAYLAND_SOCKET, removes it, and sets CLOEXEC before connecting.
        Some(
            Command::new(program)
                .args(&shell[1..])
                .process_group(0)
                .stdin(std::process::Stdio::from(std::os::fd::OwnedFd::from(
                    shell_socket,
                )))
                .env("WAYLAND_SOCKET", "0")
                .env(
                    "WAYLAND_DISPLAY",
                    std::path::PathBuf::from(
                        std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?,
                    )
                    .join(&server.socket_name),
                )
                .env("DISPLAY", format!(":{display}"))
                .spawn()?,
        )
    } else {
        None
    };
    let shell_launcher = Launcher {
        group: shell_child.as_ref().map(Pid::from_child),
        child: Rc::new(RefCell::new(shell_child)),
    };
    let child = if let Some(program) = command.first() {
        Some(
            Command::new(program)
                .args(&command[1..])
                .process_group(0)
                .env(
                    "WAYLAND_DISPLAY",
                    std::path::PathBuf::from(
                        std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?,
                    )
                    .join(&server.socket_name),
                )
                .env("DISPLAY", format!(":{display}"))
                .env_remove("WAYLAND_SOCKET")
                .spawn()?,
        )
    } else {
        None
    };
    let launcher = Launcher {
        group: child.as_ref().map(Pid::from_child),
        child: Rc::new(RefCell::new(child)),
    };
    for launcher in [&launcher, &shell_launcher] {
        if let Some(child) = launcher.child.borrow().as_ref() {
            let fd = pidfd_open(Pid::from_child(child), PidfdFlags::empty())?;
            let child = launcher.child.clone();
            server.events.handle().insert_source(
                Generic::new(fd, Interest::READ, PollMode::Level),
                move |_, _, _| {
                    if let Some(mut child) = child.borrow_mut().take() {
                        child.wait()?;
                    }
                    Ok(PostAction::Remove)
                },
            )?;
        }
    }
    // Mask before EGL/winit creates threads; the child was spawned unmasked.
    let signals = smithay::reexports::calloop::signals::Signals::new(&[
        smithay::reexports::calloop::signals::Signal::SIGTERM,
        smithay::reexports::calloop::signals::Signal::SIGINT,
        smithay::reexports::calloop::signals::Signal::SIGHUP,
    ])?;
    let running = Rc::new(std::cell::Cell::new(true));
    let signal_running = running.clone();
    server
        .events
        .handle()
        .insert_source(signals, move |_, _, _| signal_running.set(false))?;
    let (mut backend, events) = winit::init_from_attributes_with_gl_attr::<GlesRenderer>(
        Window::default_attributes()
            .with_title("Mimshel compositor")
            .with_inner_size(PhysicalSize::new(1280, 800)),
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: false,
            vsync: true,
        },
    )?;
    let dh = server.state.dh.clone();
    if let Err(error) = backend.renderer().bind_wl_display(&dh) {
        eprintln!("EGL Wayland binding unavailable: {error}");
    }
    server
        .state
        .dmabuf
        .create_global::<crate::state::State>(&dh, backend.renderer().dmabuf_formats());
    let timing = std::env::var_os("MIMSHEL_TIMING").as_deref() == Some(std::ffi::OsStr::new("1"));
    let output = server.output.clone();
    let mode = Mode {
        size: backend.window_size(),
        refresh: host_refresh(backend.window()),
    };
    server.state.output_scale = backend.scale_factor();
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(smithay::output::Scale::Fractional(
            server.state.output_scale,
        )),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    let logical = mode
        .size
        .to_f64()
        .to_logical(server.state.output_scale)
        .to_i32_round();
    server.state.model.resize_output(logical.w, logical.h);
    server.state.space.map_output(&output, (0, 0));
    let event_running = running.clone();
    let event_output = output.clone();
    server
        .events
        .handle()
        .insert_source(events, move |event, _, state| match event {
            WinitEvent::CloseRequested => event_running.set(false),
            WinitEvent::Input(event) => {
                if let Some(mode) = event_output.current_mode() {
                    state.input(
                        event,
                        mode.size
                            .to_f64()
                            .to_logical(state.output_scale)
                            .to_i32_round(),
                    );
                }
            }
            WinitEvent::Resized { size, scale_factor } if size.w > 0 && size.h > 0 => {
                state.output_scale = scale_factor;
                event_output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: event_output
                            .current_mode()
                            .map_or(60_000, |mode| mode.refresh),
                    }),
                    None,
                    Some(smithay::output::Scale::Fractional(scale_factor)),
                    None,
                );
                let logical = size.to_f64().to_logical(scale_factor).to_i32_round();
                state.model.resize_output(logical.w, logical.h);
                state.configure_windows();
            }
            WinitEvent::Redraw => state.dirty = true,
            WinitEvent::Focus(false) => state.host_focus_lost(),
            _ => {}
        })?;
    let mut tracker = OutputDamageTracker::from_output(&output);
    let start = Instant::now();
    let mut last_frame = start;
    let mut reported_mode = None;
    let result = (|| -> Result<(), Error> {
        while running.get() {
            let refresh = host_refresh(backend.window());
            if let Some(mut mode) = output.current_mode()
                && mode.refresh != refresh
            {
                mode.refresh = refresh;
                output.change_current_state(Some(mode), None, None, None);
                output.set_preferred(mode);
                server.state.dirty = true;
            }
            if timing && output.current_mode() != reported_mode {
                reported_mode = output.current_mode();
                if let Some(mode) = reported_mode {
                    eprintln!(
                        "output {}x{} @ {:.3} Hz",
                        mode.size.w,
                        mode.size.h,
                        f64::from(mode.refresh) / 1000.0
                    );
                }
            }
            for (buffer, notifier) in server.state.imports.drain(..) {
                if backend.renderer().import_dmabuf(&buffer, None).is_ok() {
                    let _ = notifier.successful::<crate::state::State>();
                } else {
                    notifier.failed();
                }
            }
            backend.window().set_cursor(server.state.cursor);
            let size = backend.window_size();
            if server.state.dirty && size.w > 0 && size.h > 0 {
                server.state.dirty = false;
                let shown = render::frame(
                    &mut backend,
                    &mut server.state,
                    &mut tracker,
                    &mut render_state,
                )?;
                // Frame callbacks at most once per refresh. A frame with
                // nothing to show (a window drawing behind another one, a
                // client that only asks for the next frame) is not held up
                // by the screen, and its callback went out at once: client
                // and compositor then ran each other flat out.
                if !shown {
                    let refresh = output
                        .current_mode()
                        .map_or(60_000, |m| m.refresh)
                        .max(1000);
                    let period = std::time::Duration::from_secs_f64(1000.0 / f64::from(refresh));
                    if let Some(left) = period.checked_sub(last_frame.elapsed()) {
                        std::thread::sleep(left);
                    }
                }
                last_frame = std::time::Instant::now();
                render::callbacks(&server.state, &output, start.elapsed());
                server.state.dh.flush_clients()?;
            }
            server.dispatch(None)?;
        }
        Ok(())
    })();
    drop(launcher);
    drop(shell_launcher);
    result
}

/// Read the current host monitor rate, including when the window changes monitors.
fn host_refresh(window: &Window) -> i32 {
    window
        .current_monitor()
        .and_then(|monitor| monitor.refresh_rate_millihertz())
        .and_then(|rate| i32::try_from(rate).ok())
        .unwrap_or(60_000)
}
