# Mimshel compositor

Phase 2.2: a separate nested Wayland compositor with server-side decorations. The X11 shell and the phase 0
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

Click to focus and raise a window. Drag our title bar (or Super + left drag)
to move it; the title bar stays below the output's top edge. A six-point
invisible outer frame resizes floating windows, with host resize cursors.
The left buttons close, minimize, and toggle maximize; double-clicking the
title bar also toggles maximize. **Super+U temporarily restores the most
recently minimized window**, until the dock provides this action. Minimized
windows remain mapped in policy but leave the desktop space and keyboard focus.
Client maximize, fullscreen, restore, and minimize requests are supported;
fullscreen hides our frame. Output size changes reconfigure maximized and
fullscreen clients.

`xdg-decoration` advertises server-side mode. An explicit client-side mode
request is respected; clients without the protocol receive our decorations.
The Colloid-inspired dark title bar uses the configured `look.font` at
Semi-Bold 10 pt, a centered title, rounded top corners, and red/yellow/green
buttons (grey when inactive). Hovering the button group reveals ×, −, +.
The font is read once at startup from `MIMSHEL_CONFIG`, or
`$XDG_CONFIG_HOME/mimshel/config.toml` (`~/.config` by default).
Title textures are cached per window by title, width, activation, hover,
and output scale. Client repaints reuse them without laying out text;
changing only the title bar damages only that element. The one-pixel side
and bottom borders have independent persistent render identities.

The keyboard has US and Russian layouts, Alt + Shift switches groups;
repeat is 25 keys/s after 600 ms. Shell protocols, XWayland, and clipboard
remain subsequent steps.

The model has no Smithay types. Protocol surfaces become model windows when
they attach a buffer, and leave the stack on unmap or destruction. Rendering
uses `OutputDamageTracker`, a solid background, and buffer age. A blocking
calloop loop waits for host input and client activity with no periodic timer.
Timing includes GPU completion and excludes presentation/vsync waiting.

`cargo test -p mimshel-compositor --locked` tests model policy and a real
Wayland connection with shm toplevels and decoration negotiation on a private temporary socket, with
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
continued with shm/dmabuf protocols. Hardware dmabuf import and rendering still need a live check. Subsequent
Radeon laptop measurements are recorded below.

## Laptop measurements and hybrid GPU setup

On a Radeon 780M, nested in X11 with kitty, render time was 2.3 ms median,
4.4 ms p95, and 8.7 ms for the first frame; idle CPU was approximately 0.1%.

GLVND loads every EGL vendor: on this laptop NVIDIA libraries accounted for
96 MB alongside 63 MB of Mesa libraries, with 184 MB total PSS. Restricting
vendor loading reduced PSS to 85 MB, of which about 16 MB was the compositor
itself. For nested runs on hybrid laptops, launch with:

```sh
__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json \
  target/debug/mimshel-compositor --nested -- kitty
```

The compositor does not change its process environment. Phase 3 will choose
the render device explicitly. `MIMSHEL_TIMING=1` is read once at startup;
only timed runs wait for GPU completion to measure render duration.
SIGTERM, SIGINT, and SIGHUP use the same orderly shutdown as closing the
host window, terminating the launched process group and removing the socket
and lock file.

## Review validation (step 2.1b)

Formatting, workspace Clippy with warnings denied, workspace tests under
Xvfb, all five ignored X11 window tests, and cargo-deny passed. The sandbox's
missing X11 keyboard libraries were supplied through `LD_LIBRARY_PATH` from
`/tmp/compositor-libs/usr/lib/x86_64-linux-gnu`; cargo-deny was run from
`/tmp/mimshel-tools/bin`.

The new process-level shutdown test verifies SIGTERM, SIGINT, and SIGHUP:
successful compositor exit, launched client termination, and removal of both
the socket and lock file. A separate Xvfb smoke run rendered a real shm
client, sent SIGTERM, and verified exit 0, the client gone, and no
`wayland-*` files remaining in its private runtime directory.

Monitor refresh is sampled from the host window at startup and after event
loop wakeups, covering resize and monitor moves without adding an idle timer.
A physical monitor transition and 165 Hz advertisement still require a live
host check; Xvfb provides only the fallback/software-rendered environment.

## Decoration snapshots

The pure `decoration::render` rasterizer and `decoration::hit_test` need no
Wayland connection or GPU. To write the 2× title bar snapshot:

```sh
MIMSHEL_DECORATION_SNAPSHOT=/tmp/mimshel-decoration-2-2.png \
  cargo test -p mimshel-compositor decoration::tests::snapshot --locked
```

Model tests cover normal/maximized/fullscreen/minimized transitions, saved
geometry, output resizing, most-recent minimize restore, and closing the
focused window above hidden windows. The protocol test checks server-side
mode, a pre-map maximize configure of 1280×770 on the 1280×800 output,
fullscreen at 1280×800, client-side opt-out, returning to server-side mode,
mapping while maximized, and a client minimize request clearing focus and
removing the window from the desktop space.

## Validation (step 2.2, 2026-10-31, sandbox)

All five requested checks passed: formatting, workspace Clippy with warnings
denied, workspace tests under Xvfb, the five ignored X11 window tests, and
cargo-deny. The existing panic and unsafe-code lints remain enabled. No shell
code or dependency versions changed. Incremental compilation was disabled
after clearing generated compiler caches to recover disk space.

Kitty 0.32.2 negotiated **server-side decorations** (`configure(2)`). An Xvfb
run with the debug binary and Mesa llvmpipe exercised the three buttons,
double-click maximize/restore, title dragging, frame resizing, kitty's real
fullscreen/unfullscreen requests, and Super+U restore while the Russian
layout was active. The maximized configure was 1280×770; fullscreen was
1280×800 with no frame. The host resize cursor shapes are supported through
winit. A separate 2× run advertised output scale 2, configured kitty at
640×370, and rendered a 60-pixel title bar with doubled button coordinates.

With cursor blinking disabled, ten idle seconds added **zero frames**
(48 → 48). SIGTERM exited successfully and removed the socket and lock file.
The final 50-frame software-rendered sample with `MIMSHEL_TIMING=1` measured
**9.11 ms median, 47.23 ms p95, 66.09 ms maximum**, including GPU completion
and decoration cache misses. This does not establish the laptop's hardware
frame budget; fractional output scale and a physical monitor transition
also need a live check.

Artifacts from the run:

- Decorated kitty: `/tmp/mimshel-compositor-2-2.png`
- Scale 2: `/tmp/mimshel-compositor-2-2-scale2.png`
- Pure title bar snapshot: `/tmp/mimshel-decoration-2-2.png`
- Fullscreen: `/tmp/22-fullscreen.png`
- Protocol/frame trace: `/tmp/22-kitty.stderr`
- Smoke assertions and timing: `/tmp/22-smoke-final.log`
