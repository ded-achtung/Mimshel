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
