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
struct Launcher(Rc<RefCell<Option<Child>>>);
impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.borrow_mut().take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = kill_process_group(Pid::from_child(&child), Signal::TERM);
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}
pub fn run(command: Vec<OsString>) -> Result<(), Error> {
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
    let mut server = Server::new()?;
    let dh = server.state.dh.clone();
    if let Err(error) = backend.renderer().bind_wl_display(&dh) {
        eprintln!("EGL Wayland binding unavailable: {error}");
    }
    server
        .state
        .dmabuf
        .create_global::<crate::state::State>(&dh, backend.renderer().dmabuf_formats());
    let output = server.output.clone();
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
    server.state.model.resize_output(mode.size.w, mode.size.h);
    server.state.space.map_output(&output, (0, 0));
    let running = std::rc::Rc::new(std::cell::Cell::new(true));
    let event_running = running.clone();
    let event_output = output.clone();
    server
        .events
        .handle()
        .insert_source(events, move |event, _, state| match event {
            WinitEvent::CloseRequested => event_running.set(false),
            WinitEvent::Input(event) => state.input(
                event,
                event_output.current_mode().unwrap().size.to_logical(1),
            ),
            WinitEvent::Resized { size, .. } if size.w > 0 && size.h > 0 => {
                event_output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                state.model.resize_output(size.w, size.h);
                state.sync_positions();
            }
            WinitEvent::Redraw => state.dirty = true,
            WinitEvent::Focus(false) => state.host_focus_lost(),
            _ => {}
        })?;
    println!("WAYLAND_DISPLAY={}", server.socket_name.to_string_lossy());
    let child = if let Some(program) = command.first() {
        Some(
            Command::new(program)
                .args(&command[1..])
                .process_group(0)
                .env("WAYLAND_DISPLAY", &server.socket_name)
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_SOCKET")
                .spawn()?,
        )
    } else {
        None
    };
    let launcher = Launcher(Rc::new(RefCell::new(child)));
    if let Some(child) = launcher.0.borrow().as_ref() {
        let fd = pidfd_open(Pid::from_child(child), PidfdFlags::empty())?;
        let child = launcher.0.clone();
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
    let mut tracker = OutputDamageTracker::from_output(&output);
    let start = Instant::now();
    let result = (|| -> Result<(), Error> {
        while running.get() {
            for (buffer, notifier) in server.state.imports.drain(..) {
                if backend.renderer().import_dmabuf(&buffer, None).is_ok() {
                    let _ = notifier.successful::<crate::state::State>();
                } else {
                    notifier.failed();
                }
            }
            let size = backend.window_size();
            if server.state.dirty && size.w > 0 && size.h > 0 {
                server.state.dirty = false;
                render::frame(&mut backend, &server.state, &mut tracker)?;
                render::callbacks(&server.state, &output, start.elapsed());
                server.state.dh.flush_clients()?;
            }
            server.dispatch(None)?;
        }
        Ok(())
    })();
    drop(launcher);
    result
}
