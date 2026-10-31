fn main() -> Result<(), mimshel_compositor::Error> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--nested")) {
        return Err("usage: mimshel-compositor --nested [-- COMMAND…]".into());
    }
    let remaining: Vec<_> = args.collect();
    if !remaining.is_empty() && remaining[0] != "--" {
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
    mimshel_compositor::backend::winit::run(remaining.into_iter().skip(1).collect())
}
