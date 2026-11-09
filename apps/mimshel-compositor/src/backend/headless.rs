//! Protocol server shared by the nested backend and GPU-free tests.

use crate::{
    Error,
    state::{ClientState, State},
};
use smithay::reexports::{
    calloop::{EventLoop, Interest, Mode as PollMode, PostAction, generic::Generic},
    wayland_server::Display,
};
use std::{os::fd::AsFd, path::Path, sync::Arc, time::Duration};

/// Wayland listener and event loop with a virtual output.
pub struct Server {
    xwayland: Option<crate::xwayland::Instance>,
    listener_token: smithay::reexports::calloop::RegistrationToken,
    display_token: smithay::reexports::calloop::RegistrationToken,
    /// Virtual nested output.
    pub output: smithay::output::Output,
    /// Shared compositor state.
    pub state: State,
    /// Blocking protocol event loop.
    pub events: EventLoop<'static, State>,
    /// Published Wayland socket basename.
    pub socket_name: std::ffi::OsString,
}

impl Server {
    /// Create initialized state.
    pub fn new() -> Result<Self, Error> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
        let runtime = std::path::PathBuf::from(runtime);
        if !runtime.is_absolute() {
            return Err("XDG_RUNTIME_DIR must be absolute".into());
        }
        for number in 1..usize::MAX {
            let path = runtime.join(format!("wayland-{number}"));
            match Socket::bind_absolute(path) {
                Ok(socket) => return Self::with_socket(socket),
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists
                        || error.kind() == std::io::ErrorKind::AddrInUse =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err("no free Wayland socket name".into())
    }

    /// Create a server on a caller-selected private socket.
    pub fn private(path: &Path) -> Result<Self, Error> {
        Self::with_socket(Socket::bind_absolute(path.to_owned())?)
    }

    /// Register the listener and Wayland display with calloop.
    fn with_socket(socket: Socket) -> Result<Self, Error> {
        let mut display = Display::<State>::new()?;
        let display_fd = display.as_fd().try_clone_to_owned()?;
        let mut dh = display.handle();
        let events = EventLoop::try_new()?;
        let mut state = State::new(dh.clone(), events.handle())?;
        use smithay::{
            output::{Mode, Output, PhysicalProperties, Subpixel},
            utils::Transform,
        };
        let output = Output::new(
            "mimshel-0".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Mimshel".into(),
                model: "Virtual output".into(),
                serial_number: String::new(),
            },
        );
        output.create_global::<State>(&dh);
        let mode = Mode {
            size: (1280, 800).into(),
            refresh: 60_000,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        state.space.map_output(&output, (0, 0));
        let name = socket.path.file_name().unwrap_or_default().to_owned();
        let listener_token = events.handle().insert_source(
            Generic::new(socket, Interest::READ, PollMode::Level),
            move |_, socket, _| {
                // An error here must not leave the event loop, which would
                // end the session for every client: a program leaking
                // connections until the file descriptors run out is its own
                // problem, not everybody's.
                loop {
                    match socket.accept() {
                        Ok(Some(stream)) => {
                            if let Err(error) =
                                dh.insert_client(stream, Arc::new(ClientState::default()))
                            {
                                eprintln!("client not accepted: {error}");
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            eprintln!("accepting a client: {error}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )?;
        let display_token = events.handle().insert_source(
            Generic::new(display_fd, Interest::READ, PollMode::Level),
            move |_, _, state| {
                display.dispatch_clients(state)?;
                Ok(PostAction::Continue)
            },
        )?;
        Ok(Self {
            xwayland: None,
            listener_token,
            display_token,
            output,
            state,
            events,
            socket_name: name,
        })
    }

    /// Reserve a display and start the server-owned XWayland process.
    pub fn start_xwayland(&mut self) -> Result<u32, Error> {
        if self.xwayland.is_none() {
            self.xwayland = Some(crate::xwayland::start(&self.state, self.events.handle())?);
        }
        self.xwayland
            .as_ref()
            .map(|x| x.display)
            .ok_or_else(|| "XWayland was not started".into())
    }

    /// Dispatch protocol events and flush replies.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        self.events.dispatch(timeout, &mut self.state)?;
        self.state.refresh();
        self.state.dh.flush_clients()?;
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.xwayland.take().is_some() {
            // The WM channel removes itself on Closed. Dispatch it after closing
            // XWayland to release Smithay's retained LoopHandle and listeners.
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !self.state.xwms.is_empty() && std::time::Instant::now() < deadline {
                if self
                    .events
                    .dispatch(Some(Duration::from_millis(10)), &mut self.state)
                    .is_err()
                {
                    break;
                }
            }
        }
        // Listener ownership must not depend on third-party callback lifetimes.
        let handle = self.events.handle();
        handle.remove(self.listener_token);
        handle.remove(self.display_token);
    }
}

// Bind atomically without unlinking stale sockets or taking an existing name.
struct Socket {
    listener: std::os::unix::net::UnixListener,
    path: std::path::PathBuf,
    lock_path: std::path::PathBuf,
    _lock: std::fs::File,
}

impl Socket {
    /// Reserve a socket name without replacing existing files.
    fn bind_absolute(path: std::path::PathBuf) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(std::io::ErrorKind::AlreadyExists.into());
        }
        let lock_path = path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock_path)?;
        smithay::reexports::rustix::fs::flock(
            &lock,
            smithay::reexports::rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )?;
        let listener = match std::os::unix::net::UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = std::fs::remove_file(&lock_path);
                return Err(error);
            }
        };
        let socket = Self {
            listener,
            path,
            lock_path,
            _lock: lock,
        };
        socket.listener.set_nonblocking(true)?;
        Ok(socket)
    }

    /// Accept one client without blocking.
    fn accept(&self) -> std::io::Result<Option<std::os::unix::net::UnixStream>> {
        match self.listener.accept() {
            Ok((stream, _)) => Ok(Some(stream)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl AsFd for Socket {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.listener.as_fd()
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_socket_name_is_never_replaced() {
        let runtime = tempfile::tempdir().unwrap();
        let path = runtime.path().join("wayland-1");
        std::fs::write(&path, b"leave this name alone").unwrap();
        assert!(Socket::bind_absolute(path.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"leave this name alone");
        let free = runtime.path().join("wayland-2");
        let socket = Socket::bind_absolute(free.clone()).unwrap();
        assert!(Socket::bind_absolute(free.clone()).is_err());
        assert!(
            smithay::reexports::wayland_server::ListeningSocket::bind_absolute(free.clone())
                .is_err()
        );
        drop(socket);
        assert!(!free.exists());
        assert!(!free.with_extension("lock").exists());
    }
}
