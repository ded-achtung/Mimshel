# Mimshel's compositor (Wayland plan, phase 2)

Phase 2 of `docs/wayland-plan.md`: our own Wayland compositor on Smithay,
first **nested** — it runs in an ordinary window inside today's X11 session,
programs and our shell are started inside it. The X11 session stays the
working one; nothing on it changes in this phase. Branch `platform/boundary`
(not merged into master until the user says so).

## Shape

- **A separate program, `apps/mimshel-compositor`** (binary
  `mimshel-compositor`). The compositor must stay small and robust — its crash
  ends the whole Wayland session — and the shell binary should not link
  Smithay. In phase 3 the session starts the compositor, which starts
  `mimshel daemon` as an ordinary Wayland client.
- **Inside it** (each a module, dependencies only downwards):
  - `model` — windows as plain data: stacking order, focus, positions, sizes,
    placement of new windows. **No Smithay types**, fully unit-tested.
  - `state` — Smithay state and the protocol handlers; turns protocol events
    into `model` calls and back.
  - `input` — keyboard (xkb keymap: layouts and the switch key from Mimshel's
    config, us+ru by default), pointer, grabs for moving and resizing.
  - `render` — GLES through Smithay's `GlesRenderer`, damage-tracked: only
    changed regions are redrawn, nothing at all when nothing changed.
  - `backend` — `winit` (nested, phase 2) and `headless` (no output, for
    tests); `drm` comes in phase 3.
- **Rules**: no `unsafe` (Smithay holds what needs it); English comments;
  the frame budget of `docs/wayland-plan.md` (render ≤ 2–3 ms; `MIMSHEL_TIMING=1`
  prints frame times); the cost rule of `docs/platform.md` (no work when idle:
  an idle compositor draws no frames and wakes only for input or clients;
  memory flat over time).
- **Tests**: `model` by unit tests; the protocols through the `headless`
  backend with a real `wayland-client` connection in the test (create a
  toplevel, check focus and stacking) — runs in CI without a GPU.

## Steps (each ends with a check on the laptop)

| # | Step | Who |
|---|---|---|
| 2.1 | Skeleton: winit and headless backends, xdg-shell windows, keyboard and pointer, focus on click, stacking, moving (Super + drag and the client's own move request), resizing (client request), damage-tracked rendering, frame timing. kitty runs inside. | Codex — **done** |
| 2.2 | Window decorations of our own (server side, `xdg-decoration`): title bar with the three buttons on the left, as on X11 today; maximize and minimize, fullscreen. | Codex — **done** |
| 2.3 | Protocols for the shell: `wlr-layer-shell`, `ext-foreign-toplevel-list` + `wlr-foreign-toplevel-management`, `xdg-activation`, `ext-session-lock-v1`, `ext-idle-notify-v1`. | Codex — **done** |
| 2.4 | The door's Wayland backend (`platform.rs` gets the second variant): the menu bar, the dock and the desktop run inside the nested compositor. | Claude — **under way**: `crates/wayland`; our terminal and the menu bar run inside (2026-11-01) |
| 2.5 | XWayland: Houdini, Blender, Chrome, Telegram, Steam inside (needs `xorg-xwayland` on the laptop). | Codex — **done** (started at launch for now, ~29 MB; to start on first use) |
| 2.6 | Clipboard and drag and drop between programs and our windows; keyboard layout switching. | — |

## Trying it

`tools/nested-session.sh` (after `cargo build --release`) opens the compositor
in a window and starts inside it a second, isolated Mimshel service with only
the menu bar (own runtime, state and cache folders, own config copy, own
D-Bus), our terminal as a Wayland client and the same terminal as an X11
program through XWayland. Closing the compositor's window ends everything.

On Wayland the door has so far: app windows, panels (layer shell with an
exclusive zone), popups (overlay layer), keyboard, pointer, outputs. Not yet:
the window list (foreign-toplevel client — the menu bar shows no active app),
keyboard layouts, clipboard, drag and drop, the desktop and the lock screen.

## Measurements

Laptop (Radeon 780M, nested in X11, kitty inside, Mesa EGL only —
`__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`),
2026-10-31, after step 2.1:

| | |
|---|---|
| Frame render | median 1.35 ms, p95 2.6 ms, max 4.4 ms |
| Idle | CPU ~0.1 %; frames only for kitty's blinking cursor |
| Memory (PSS) | 69 MB, flat; ~16 MB of it is the compositor, the rest Mesa (LLVM, gallium) |
| Without the EGL vendor choice | 184 MB: GLVND also loads NVIDIA's EGL (96 MB) |
| `kill -TERM` | the client is stopped, no `wayland-*` left behind |

After step 2.3 (2026-11-01, our own terminal inside, decorations on): render
median 2.1 ms; idle 20 s → 0 frames, 0 CPU ticks. PSS 148 MB, but read it with
care: PSS splits shared pages among the processes using them, and right after a
reboot few others map Mesa's `libLLVM`, so its share alone went 49 → 96 MB. What
is the compositor's own: anonymous memory (`Pss_Anon`) 20 MB, the binary 7.6 MB,
the title font 2 MB (only the configured family is loaded now). Compare
`Pss_Anon` between builds, not the total.

## Step 2.3 — shell protocols

The nested compositor publishes these globals; the shell's Wayland clients are
still separate work. The only change in `apps/mimshel` is delegation of its
existing config-path function to core; no client protocol behavior changes.

- `zwlr_layer_shell_v1`: Smithay's layer surfaces and output layer map arrange
  anchors, margins and exclusive zones. Rendering and pointer hit-testing use
  overlay, top, windows, bottom, background order. A committed positive exclusive
  zone reduces the model's work area, including new window placement and existing
  maximized windows. Fullscreen uses the complete output. NULL-buffer unmapping
  releases the reservation. Top/overlay exclusive keyboard focus overrides window
  focus; on-demand focus follows a click and survives repaints; noninteractive
  layers receive no keyboard focus. Layer popups are tracked with PopupManager.
- `ext_foreign_toplevel_list_v1`: Smithay handles publish mapped windows with
  title, app id and stable identifiers, update metadata, and close on unmap.
  Minimized windows remain listed. `zwlr_foreign_toplevel_manager_v1` has a small
  compositor dispatcher (Smithay 0.7 has no helper). It publishes metadata,
  output membership and presentation state, and accepts activate, close,
  minimize/restore, maximize/unmaximize and fullscreen requests. Restoration
  preserves a minimized window's previous presentation mode. Unchanged windows
  send no repeated events, and ordinary repaint commits do not rebuild the list.
- `xdg_activation_v1`: Smithay issues tokens; activation consumes the token,
  rejects tokens older than ten seconds, and restores/raises/focuses a mapped
  target. Tokens are bearer capabilities; this nested development compositor
  does not require an input serial when issuing one. Activation is suppressed
  while locked.
- `ext_session_lock_manager_v1`: accepting a lock immediately switches both
  rendering and input to a lock-only scene, clears window grabs, and clears
  pointer/keyboard focus. The `locked` event follows presentation of that scene.
  Its clear color is opaque black. Lock-client death clears its surfaces and
  input focus, **never the locked flag**. Another locker may take over; only the
  accepted owner can unlock. Lock surfaces are configured for output size and
  reconfigured on resize. Background clients cannot take popup grabs while
  locked. A rejected concurrent locker cannot unlock the active lock.
- `ext_idle_notifier_v1`: Smithay uses calloop deadline timers. Actual seat input
  resets them and emits `resumed`; reaching a deadline emits `idled`. Neither
  notification requests a render frame, and there is no idle polling timer.

`tests/shell.rs` uses real socket-connected `wayland-client` connections and a
GPU-free headless server. It checks an exclusive top panel's placement/maximize
bounds, fullscreen and layer removal; both foreign lists, app ids, raising and
single-use activation, management requests; none/on-demand/exclusive keyboard
focus and NULL-buffer unmapping; idle timeout and reset through the actual input
handler. The session-lock test keeps an ordinary mapped window underneath,
checks the **same scene source list consumed by GLES** before every simulated
presentation, verifies that all entries are lock surfaces, disconnects the
locker, verifies an empty scene and no input target while still locked, then
locks and unlocks with a replacement client. This is a scene-selection test;
headless presentation does not exercise EGL or host-window swap timing.

The compositor still has one nested output. Multi-output placement, physical
scanout and locker recovery on actual DRM outputs belong to the DRM backend;
this step does not validate them.

### Decoration font measurement

The renderer now reads validated configuration through `mimshel_core::config`
and uses `model.title_height(id)` for title texture geometry and border offsets.
Configuration path resolution has a pure precedence test (override, absolute
XDG path, HOME fallback, missing HOME). The shell retains its existing writing,
migration and backup policy.

Title fonts use only the configured family plus fontconfig's resolved sans-serif
fallback, instead of cosmic-text's full system database. A temporary debug
probe constructed each font set, measured `Terminal — Терминал` at 13 px/600,
and read `/proc/self/smaps_rollup`: **5,433 KiB PSS full vs 4,907 KiB restricted**,
a saving of **526 KiB** on this server. The server has only eight fontconfig
entries and no Inter; this measures fallback behavior and is not a laptop PSS
comparison. Repeat the complete compositor measurement on the laptop's font set
before attributing any of its 69 → 91 MB increase to fonts. The probe and its
source were removed after measurement.

Validation on the server: `cargo fmt --all -- --check`, workspace/all-targets
Clippy with `-D warnings`, the locked workspace tests under Xvfb, the ignored
`mimshel-x11` window suite under Xvfb, and `cargo deny --locked check` all pass.
The audit used a temporary cargo-deny 0.20.2 installation (removed afterward).
All builds used `CARGO_INCREMENTAL=0`; the existing target directory was reused.
