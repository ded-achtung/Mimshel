//! Process-level shutdown coverage; requires an X server (for example Xvfb).
use smithay::reexports::rustix::process::{Pid, Signal, kill_process};
use std::{
    process::Command,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{ConnectionExt, CreateWindowAux, MapState, WindowClass},
};

#[test]
fn signals_remove_socket_and_terminate_client() {
    if std::env::var_os("DISPLAY").is_none()
        || std::process::Command::new("Xwayland")
            .arg("-version")
            .output()
            .is_err()
    {
        eprintln!("SKIP: shutdown test needs a host DISPLAY and Xwayland");
        return;
    }
    for signal in [Signal::TERM, Signal::INT, Signal::HUP] {
        let runtime = tempfile::tempdir().unwrap();
        let pid_file = runtime.path().join("client.pid");
        let mut compositor = Command::new(env!("CARGO_BIN_EXE_mimshel-compositor"))
            .args([
                "--nested",
                "--",
                "/bin/sh",
                "-c",
                "echo $$ > \"$1\"; echo \"$DISPLAY\" > \"$1.display\"; exec sleep 60",
                "client",
            ])
            .arg(&pid_file)
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .spawn()
            .unwrap();
        // Wait for initialization rather than signalling during process startup.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let ready = std::fs::read_to_string(format!("/proc/{}/status", compositor.id()))
                .is_ok_and(|status| {
                    status
                        .lines()
                        .find(|line| line.starts_with("Threads:"))
                        .and_then(|line| line.split_whitespace().nth(1))
                        .and_then(|n| n.parse::<usize>().ok())
                        .is_some_and(|n| n > 1)
                });
            assert!(
                compositor.try_wait().unwrap().is_none(),
                "compositor exited during initialization"
            );
            if ready && pid_file.exists() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = compositor.kill();
                panic!("initialization timed out");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let client: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let display = std::fs::read_to_string(pid_file.with_extension("pid.display")).unwrap();
        let number = display.trim().strip_prefix(':').unwrap();
        let (connection, screen) = x11rb::connect(Some(display.trim())).unwrap();
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                connection.setup().roots[screen].root,
                0,
                0,
                100,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap();
        connection.map_window(window).unwrap();
        connection.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while connection
            .get_window_attributes(window)
            .unwrap()
            .reply()
            .unwrap()
            .map_state
            != MapState::VIEWABLE
        {
            assert!(Instant::now() < deadline, "X11 window was not mapped");
            std::thread::sleep(Duration::from_millis(20));
        }
        let x_socket = format!("/tmp/.X11-unix/X{number}");
        let x_lock = format!("/tmp/.X{number}-lock");
        assert!(std::path::Path::new(&x_socket).exists());
        assert!(std::path::Path::new(&x_lock).exists());
        let children = std::fs::read_to_string(format!(
            "/proc/{}/task/{}/children",
            compositor.id(),
            compositor.id()
        ))
        .unwrap();
        let xwayland = children
            .split_whitespace()
            .find(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .is_ok_and(|name| name.trim() == "Xwayland")
            })
            .map(str::to_owned)
            .expect("XWayland child");
        kill_process(Pid::from_child(&compositor), signal).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while compositor.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = compositor.kill();
                panic!("shutdown timed out");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(compositor.wait().unwrap().success());
        assert!(!std::path::Path::new(&x_socket).exists());
        assert!(!std::path::Path::new(&x_lock).exists());
        let deadline = Instant::now() + Duration::from_secs(30);
        while std::path::Path::new(&format!("/proc/{xwayland}")).exists() {
            assert!(
                Instant::now() < deadline,
                "XWayland survived compositor shutdown"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!std::path::Path::new(&format!("/proc/{client}")).exists());
        assert!(!runtime.path().join("wayland-1").exists());
        assert!(!runtime.path().join("wayland-1.lock").exists());
    }
}
