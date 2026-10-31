//! Process-level shutdown coverage; requires an X server (for example Xvfb).
use smithay::reexports::rustix::process::{Pid, Signal, kill_process};
use std::{
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn signals_remove_socket_and_terminate_client() {
    if std::env::var_os("DISPLAY").is_none() {
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
                "echo $$ > \"$1\"; exec sleep 60",
                "client",
            ])
            .arg(&pid_file)
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .spawn()
            .unwrap();
        // Wait for initialization rather than signalling during process startup.
        let deadline = Instant::now() + Duration::from_secs(20);
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
        kill_process(Pid::from_child(&compositor), signal).unwrap();
        while compositor.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = compositor.kill();
                panic!("shutdown timed out");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(compositor.wait().unwrap().success());
        assert!(!std::path::Path::new(&format!("/proc/{client}")).exists());
        assert!(!runtime.path().join("wayland-1").exists());
        assert!(!runtime.path().join("wayland-1.lock").exists());
    }
}
