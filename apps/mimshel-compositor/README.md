# Mimshel compositor

Phase 2.1: a separate nested Wayland compositor. The X11 shell and the phase 0
probe are unchanged. Requires an X11 session, an absolute `XDG_RUNTIME_DIR`,
xkbcommon, Wayland, EGL and GLES (Mesa llvmpipe works without a GPU).

```sh
cargo build -p mimshel-compositor --locked
MIMSHEL_TIMING=1 target/debug/mimshel-compositor --nested -- kitty
```

The compositor prints `WAYLAND_DISPLAY=wayland-N`. Existing socket names and
lock files are skipped, including stale ones. The launched command receives
this display and no `DISPLAY` or inherited `WAYLAND_SOCKET`. Closing the host
window terminates the launched command. Without a command, launch clients
separately using the printed display name.

Click to focus and raise a window. Super + left drag moves it. Client-side
title bars can request moves and edge resizes. The keyboard has US and Russian
layouts, Alt + Shift switches groups; repeat is 25 keys/s after 600 ms. The
host cursor remains visible. Server decorations, shell protocols, XWayland,
clipboard and maximize/minimize/fullscreen are subsequent steps.

The model has no Smithay types. Protocol surfaces become model windows when
they attach a buffer, and leave the stack on unmap or destruction. Rendering
uses `OutputDamageTracker`, a solid background, and buffer age. A blocking
calloop loop waits for host input and client activity with no periodic timer.
Timing includes GPU completion and excludes presentation/vsync waiting.

`cargo test -p mimshel-compositor --locked` tests model policy and a real
Wayland connection with two shm toplevels on a private temporary socket, with
no output, GPU or X server. The private socket avoids changing the test
process's environment. An ignored `nested_client` test is a stationary shm
client for manual Xvfb smoke runs:

```sh
cargo test -p mimshel-compositor --test headless --no-run --locked
# Use the headless-* executable printed by Cargo:
xvfb-run -a target/debug/mimshel-compositor --nested -- \
  target/debug/deps/headless-HASH --exact nested_client --ignored --nocapture
```

## Validation (2026-10-31, sandbox)

All requested checks passed with Rust 1.99.1:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `xvfb-run -a cargo test --workspace --locked`
- `xvfb-run -a cargo test -p mimshel-x11 --test windows --locked -- --ignored`
- `cargo deny --locked check` (cargo-deny 0.20.2)

The license allow list is unchanged. Smithay 0.7.0 requires the unmaintained
`cgmath` crate; `deny.toml` documents a narrow exception for RUSTSEC-2026-0196,
which reports no vulnerability and has no safe upgrade for this dependency.
No existing dependency version was removed or upgraded.

Nested smoke runs used Xvfb, Mesa llvmpipe, the debug binary, and first a pair
of real shm clients, then kitty 0.32.2. Missing host libraries and kitty were
extracted from Ubuntu packages into `/tmp`, without changing the system.
A private D-Bus session and writable temporary shader cache were provided.
Verified the two-window image, Super + drag, kitty's own title-bar move
request and bottom-right resize request (640x400 -> 730x450), and normal
host-window shutdown (exit 0). A terminal command received `hello` and then
`ф` after Alt + Shift and the physical A key. The existing `wayland-1` file
was preserved; the compositor selected `wayland-2`.

With kitty's cursor blink disabled, a 60-second idle interval added **zero
frames** (6 -> 6) and zero process CPU ticks. PSS stayed at 83,534–83,535 KiB,
with 11 threads and 27 open file descriptors throughout. No periodic timer
is registered by the compositor.

In that software-rendered run, initial frame times were 73.7 and 53.1 ms;
later active frames were 3.7–20.6 ms, with a 31.3 ms movement spike. These
include GPU/software-renderer completion and exclude presentation waiting.
This is not a measurement of the laptop's 2–3 ms hardware frame budget.
The Xvfb EGL implementation lacks `EGL_WL_bind_wayland_display`; the server
continued with shm/dmabuf protocols. Hardware dmabuf import and rendering,
real Radeon performance, and the laptop session still need a live check.
