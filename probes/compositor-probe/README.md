# Compositor performance probe (phase 0)

Standalone, deliberately small Smithay **=0.7.0** compositor. It runs inside an
X11 window through **winit**, using the backend's ready-made `GlesRenderer`.
It does not use Smithay's X11 backend or change the Mimshel workspace/session.
The edition 2024 crate forbids unsafe code and never mutates its process environment. Its empty `[workspace]` and own Cargo.lock keep
it independent of the application.

It draws, back to front:

- A full-window GLSL star/nebula shader, driven by elapsed time.
- Real xdg-shell windows, including subsurfaces and basic popups.
- Eight colored, rounded-square dock icons. Each 112×112 premultiplied RGBA
  image is generated and uploaded **once at startup**. Per-frame zoom only
  changes texture destination rectangles (up to 1.8×), with smooth falloff,
  exponential animation, and expanding spacing.

Click a window to raise and focus it; keyboard and wheel events are forwarded.
Hold **Super + left drag** to move it. The host X11 cursor is used. There are
no window decorations, resize handles, workspaces, XWayland or session features.
Popup positioning/repositioning works, but modal popup grabs are omitted.
The dock is a visual performance layer; icons do not launch applications.

## Build and checks

Requires a Rust toolchain, a C toolchain, pkg-config, and development libraries
for Wayland, EGL/GLES and xkbcommon. On Debian/Ubuntu the relevant packages are
`build-essential pkg-config libwayland-dev libegl1-mesa-dev libgles2-mesa-dev
libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev`.
Runtime requires a current X11 session (`DISPLAY`) and an existing writable
`XDG_RUNTIME_DIR`, normally supplied by your login session.

From the repository root:

```sh
cd probes/compositor-probe
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
```

No GPU/display is needed for these checks. Tests cover magnification bounds,
symmetry, smooth edges, frame-rate-independent animation, icon spacing, and
average/nearest-rank percentiles, including empty and singleton samples. A
regression test checks independent render/present accounting and summary context.

## Measurements and interpretation

Every rendered frame uses `std::time::Instant`:

- `render`: wall-clock time from the start of dock/frame construction and
  client texture imports for rendering, through `Frame::finish()`, waiting on
  its returned `SyncPoint`. This stops before presentation.
- `present`: the `WinitGraphicsBackend::submit()` call alone (eglSwapBuffers),
  including any wait for vblank.
- `frame`: start-to-start interval between rendered frames. The first frame
  has no interval sample.

Once a second it prints `fps=… render avg=…ms p95=…ms max=…ms present avg=…ms p95=…ms max=…ms frame avg=…ms`.
`--bench SECONDS` animates the wallpaper and sweeps a synthetic dock pointer
left/right without input. It prints aggregate frames, avg/p50/p95/p99/max for both render and present,
and frame avg, then exits successfully. Each summary includes the final actual
window size, pacing derived from the launch environment, and wallpaper/dock
flags, so pasted results are self-describing. Avoid resizing during comparisons.
Samples include startup/warmup frames;
shader compilation and icon generation/upload happen before the benchmark.
Percentiles use nearest rank: sorted sample at `ceil(p*n)`, counting from one.
Benchmark samples are retained until exit; interactive statistics reset each
second to keep memory bounded.

**Compare `render` against the ~2–3 ms compositor budget.** Presentation waits
are reported separately in `present`; they do not inflate `render` in vsync mode.
Use unthrottled runs to compare raw throughput and normal pacing for smoothness.
The explicit fence wait makes this a conservative, serialized CPU+GPU completion
measurement, not GPU timer-query instrumentation or an optimized pipelined
production compositor. X server/compositor/driver overhead and contention with
clients influence the results. Client dispatch, dmabuf validation, host pointer
queries, frame callback delivery and statistics printing are outside `render`,
but are included in the next frame interval. Every frame redraws the whole
window, even with both layers disabled; this intentionally measures continuous
composition, without damage skipping, occlusion optimization or direct scanout.

**Pacing on the target Radeon/Mesa driver:** Smithay 0.7's `GlAttributes::vsync`
only selects an EGL configuration; it does not call `eglSwapInterval`. This
probe only reads the launch environment. Leave `vblank_mode` unset for the
normal driver-default vsync path, or launch with `vblank_mode=0` for unthrottled
mode. Startup and summary print `vsync(driver-default)`,
`unthrottled(vblank_mode=0)`, or `driver-configured(vblank_mode=VALUE)` for other
values; these describe the requested driver behavior, not verified presentation.
The optional `--unthrottled` flag only checks that `vblank_mode=0` is set and
exits with a diagnostic otherwise. It does not change pacing itself.
If `WAYLAND_DISPLAY` is set (even empty), startup exits with
`unset WAYLAND_DISPLAY so winit uses X11`. Launch from an X11 session where it
is absent, or use `env -u WAYLAND_DISPLAY` in your shell. There are no sleeps or
callback waits in the render loop; Wayland client callbacks are sent after
every submitted frame in either mode. Mesa overrides are not portable to other
drivers: confirm actual FPS and host refresh on the laptop. Host composition
may still constrain presentation even in unthrottled mode. The advertised
nested Wayland output refresh is nominally 60 Hz; actual pacing comes from the
host. Select 60/120 Hz in the host session before comparing those modes.

Smithay's winit adapter discards host cursor-leave events. A safe `x11rb`
query of the winit window checks pointer bounds outside render timing, so the
dock animates back down when the pointer leaves. This is input bookkeeping,
not Smithay's X11 graphics backend. It is skipped during benchmarks.

## Run matrix

Run these commands from `probes/compositor-probe` in the laptop's X11 session,
with `WAYLAND_DISPLAY` and `vblank_mode` initially unset.
Close the window to end an interactive run. `--size` requests **physical pixel**
window contents; the host window manager may constrain large windows. Always
check the startup `size=` line against the intended resolution. The size
examples below are windowed, fullscreen-sized render targets, not an exclusive
fullscreen mode. A host screen at least as large is needed for an honest 4K test.

```sh
# Normal vsync, 1280x800, both layers
./target/release/compositor-probe --size 1280x800 --bench 30

# Raw cost, same scene
vblank_mode=0 ./target/release/compositor-probe --size 1280x800 --unthrottled --bench 30

# Fullscreen-sized laptop target, then 4K; repeat both pacing modes
./target/release/compositor-probe --size 1920x1200 --bench 30
vblank_mode=0 ./target/release/compositor-probe --size 1920x1200 --unthrottled --bench 30
./target/release/compositor-probe --size 3840x2160 --bench 30
vblank_mode=0 ./target/release/compositor-probe --size 3840x2160 --unthrottled --bench 30

# Isolate wallpaper, dock, and baseline (both layers is the default above)
vblank_mode=0 ./target/release/compositor-probe --size 1280x800 --unthrottled --bench 30 --no-dock
vblank_mode=0 ./target/release/compositor-probe --size 1280x800 --unthrottled --bench 30 --no-wallpaper
vblank_mode=0 ./target/release/compositor-probe --size 1280x800 --unthrottled --bench 30 --no-wallpaper --no-dock

# Complete layer/pacing/resolution matrix
for size in 1280x800 1920x1200 3840x2160; do
  for pacing in vsync unthrottled; do
    for layers in '' '--no-wallpaper' '--no-dock' '--no-wallpaper --no-dock'; do
      if [ "$pacing" = unthrottled ]; then
        vblank_mode=0 ./target/release/compositor-probe --size "$size" --bench 30 --unthrottled $layers
      else
        env -u vblank_mode ./target/release/compositor-probe --size "$size" --bench 30 $layers
      fi
    done
  done
done
```

Keep the host compositor configuration, power mode, refresh rate and GPU
selection constant. Compare repeated runs, especially p95/p99. A VM with
Mesa software rendering can use the same commands prefixed with
`LIBGL_ALWAYS_SOFTWARE=1`; its numbers are a software-rendering floor.

## Real clients

Startup prints `WAYLAND_DISPLAY=wayland-probe-N`. `--spawn CMD` can be repeated;
it executes a shell command after the socket and protocols are ready, with
`WAYLAND_DISPLAY` set to the new socket, `GDK_BACKEND=wayland`, and `DISPLAY`
removed so a test cannot silently fall back to X11. Only the child command
environment is configured; the probe process environment remains unchanged.
The host process still uses its original X11 DISPLAY. Commands should remain in the foreground:
remaining foreground launchers and their process groups are terminated on probe
exit, and direct children are reaped.

```sh
# Interactive focus, typing, dragging, and scrolling checks
./target/release/compositor-probe --spawn 'kitty -o linux_display_server=wayland' --spawn 'foot'
./target/release/compositor-probe --spawn 'GDK_BACKEND=wayland gtk4-demo'

# Two independent kitty clients continuously scrolling text, with both layers
vblank_mode=0 ./target/release/compositor-probe --size 1280x800 --unthrottled --bench 30 \
  --spawn 'kitty -o linux_display_server=wayland sh -c '\''while :; do printf "client A: scrolling text 0123456789\n"; sleep 0.01; done'\''' \
  --spawn 'kitty -o linux_display_server=wayland sh -c '\''while :; do printf "client B: scrolling text abcdefghij\n"; sleep 0.01; done'\'''

# Repeat with normal vsync
./target/release/compositor-probe --size 1280x800 --bench 30 \
  --spawn 'kitty -o linux_display_server=wayland sh -c '\''while :; do printf "client A: scrolling text 0123456789\n"; sleep 0.01; done'\''' \
  --spawn 'kitty -o linux_display_server=wayland sh -c '\''while :; do printf "client B: scrolling text abcdefghij\n"; sleep 0.01; done'\'''
```

Alternatively run `WAYLAND_DISPLAY=wayland-probe-N foot` or
`WAYLAND_DISPLAY=wayland-probe-N GDK_BACKEND=wayland gtk4-demo` in another
terminal, substituting the printed socket name. For GTK applications that
reuse an existing process, use a fresh application instance (e.g. gtk4-demo)
so it connects to this display. Click both windows during the interactive
check and type distinct text to confirm focus delivery. Initial windows are
640×400, staggered so both scrolling clients are visible.

Client buffers support wl_shm, legacy EGL/wl_drm when driver binding succeeds,
and linux-dmabuf v3 with the renderer's advertised formats. Dmabufs are validated
with `ImportDma::import_dmabuf` before acknowledging import. A failed EGL binding
prints a diagnostic; dmabuf and shared-memory paths remain available. Actual
kitty/foot/GTK compatibility needs a display/GPU run.

## Rendering APIs and scope

- `winit::init_from_attributes_with_gl_attr::<GlesRenderer>` provides the safe,
  initialized renderer; no manual EGL context or raw OpenGL calls.
- `GlesRenderer::compile_custom_pixel_shader` compiles `src/wallpaper.frag`.
  `GlesFrame::render_pixel_shader_to` supplies its `time` uniform each frame.
- `ImportMem::import_memory` uploads icon images once as `Fourcc::Abgr8888`;
  `GlesFrame::render_texture_from_to` scales these existing textures.
- `Window::render_elements` / `WaylandSurfaceRenderElement::draw` import and
  draw client surface trees, and `Window::send_frame` releases client pacing.

The upstream [anvil](https://github.com/Smithay/smithay/tree/v0.7.0/anvil) and
[smallvil](https://github.com/Smithay/smithay/tree/v0.7.0/smallvil) illustrate
Smithay's protocol/renderer wiring. This probe is its own small implementation;
no upstream compositor is vendored.

No GPU or display is available on the implementation server. Build, lint and
pure unit tests can be verified here; shader compilation, actual presentation,
client behavior and the performance go/no-go **must be verified on the laptop**.
Timing is printed to stdout; an on-screen timing overlay from the wider phase 0
plan is not implemented. This probe follows the task's requested console metrics.
