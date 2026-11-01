#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
fn main() -> Result<(), mimshel_compositor::Error> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--nested")) {
        return Err("usage: mimshel-compositor --nested [--shell COMMAND…] [-- COMMAND…]".into());
    }
    let mut remaining: Vec<_> = args.collect();
    let shell = if remaining.first().is_some_and(|arg| arg == "--shell") {
        remaining.remove(0);
        let end = remaining
            .iter()
            .position(|arg| arg == "--")
            .unwrap_or(remaining.len());
        let shell: Vec<_> = remaining.drain(..end).collect();
        if shell.is_empty() {
            return Err("--shell requires a command".into());
        }
        shell
    } else {
        Vec::new()
    };
    if remaining.first().is_some_and(|arg| arg != "--") {
        return Err("expected -- before command".into());
    }
    if std::env::var_os("DISPLAY").is_none() {
        return Err("--nested requires an X11 DISPLAY".into());
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("WAYLAND_SOCKET").is_some()
    {
        // Re-exec with an X11-only environment; changing the process environment
        // in place would require unsafe code on edition 2024.
        use std::os::unix::process::CommandExt;
        return Err(std::process::Command::new(std::env::current_exe()?)
            .args(std::env::args_os().skip(1))
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .exec()
            .into());
    }
    mimshel_compositor::backend::winit::run(shell, remaining.into_iter().skip(1).collect())
}
