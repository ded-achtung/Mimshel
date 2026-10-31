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

pub struct Server {
    pub output: smithay::output::Output,
    pub state: State,
    pub events: EventLoop<'static, State>,
    pub socket_name: std::ffi::OsString,
}
impl Server {
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
    pub fn private(path: &Path) -> Result<Self, Error> {
        Self::with_socket(Socket::bind_absolute(path.to_owned())?)
    }
    fn with_socket(socket: Socket) -> Result<Self, Error> {
        let mut display = Display::<State>::new()?;
        let display_fd = display.as_fd().try_clone_to_owned()?;
        let mut dh = display.handle();
        let mut state = State::new(dh.clone())?;
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
        let events = EventLoop::try_new()?;
        let name = socket.path.file_name().unwrap_or_default().to_owned();
        events.handle().insert_source(
            Generic::new(socket, Interest::READ, PollMode::Level),
            move |_, socket, _| {
                while let Some(stream) = socket.accept()? {
                    dh.insert_client(stream, Arc::new(ClientState::default()))?;
                }
                Ok(PostAction::Continue)
            },
        )?;
        events.handle().insert_source(
            Generic::new(display_fd, Interest::READ, PollMode::Level),
            move |_, _, state| {
                display.dispatch_clients(state)?;
                Ok(PostAction::Continue)
            },
        )?;
        Ok(Self {
            output,
            state,
            events,
            socket_name: name,
        })
    }
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        self.events.dispatch(timeout, &mut self.state)?;
        self.state.refresh();
        self.state.dh.flush_clients()?;
        Ok(())
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
