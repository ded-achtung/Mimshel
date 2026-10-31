# Moving Mimshel to Wayland — decision and plan

> **Status: phase 1 under way (2026-10-31, the user's decision)** — see
> `docs/platform.md`. It was postponed on 2026-10-26 to finish the X11 work
> first. Phase 0 is done (GO, results below).

**Decided by the user on 2026-10-26.** This replaces two earlier decisions in
CLAUDE.md: "X11 only, not Wayland" and "windows are drawn by xfwm4, we don't
write our own window manager".

## Decision

1. Mimshel gets its **own Wayland compositor**, built on **Smithay** (Rust;
   used by COSMIC and niri). On Wayland the compositor *is* the display server,
   window manager and compositor in one program, so no X11 window manager is
   written — it would be thrown away later.
2. **Rendering on the GPU with OpenGL ES** (Smithay's `GlesRenderer`), with a
   **layer model like macOS Core Animation**: windows, dock, menu bar and
   wallpaper are textures; their *contents* are re-rendered only when they
   change, animations (dock zoom, minimise, fades) only move/scale/blend
   textures. Our widgets keep drawing their contents with `tiny-skia` on the
   CPU for now (measured: a dock frame is 1–2 ms, and only on change); a GPU
   vector renderer (e.g. `vello`) can replace that behind the same interface
   if measurements ask for it.
3. **A frame budget from day one**, like Unreal's `stat unit`: the compositor
   spends at most ~2–3 ms per frame, so 120 Hz has headroom. Frames follow
   vblank; only damaged regions are redrawn; fullscreen apps and games go
   to the screen directly (direct scanout).
4. **No `unsafe` in our code** stays the rule: GPU and KMS access live inside
   Smithay, like in our other dependencies.
5. X11 programs (Houdini, Blender, Steam…) run through **XWayland**.
6. **Until the new session is better, X11 + Xfce stays the working session.**
   The switch is a second session entry in the login screen,
   "Mimshel (Wayland)" — choosing the old one is the way back.

## Shape

- **The compositor is small and robust.** On Wayland its crash ends the whole
  session with every open program (much worse than a dock crash, lesson 12).
  It manages windows, input, outputs, rendering, the lock screen surface and
  the protocols below — nothing else.
- **The shell stays a separate process** (today's service: menu bar, dock,
  notifications, desktop, search, file manager windows), talking to the
  compositor through Wayland protocols. A shell crash does not touch open
  windows; the guard restarts it as today.
- **A platform boundary inside the shell**: the shell parts already have no
  X11 (only `apps/mimshel` and `crates/x11` use it, ~240 call sites in the
  `*_ui.rs` files). They get a small platform interface (surfaces/popups,
  input, outputs, clipboard, drag and drop, window list) with two
  implementations: X11 (today's code) and Wayland.

## What each X11 mechanism becomes

| Today (X11) | On Wayland |
|---|---|
| dock / menu bar windows (`_NET_WM_WINDOW_TYPE_DOCK`, struts) | `wlr-layer-shell` surfaces with exclusive zones |
| popups (override-redirect, grabs) | `xdg_popup` with grabs, or layer-shell overlays |
| window list for the dock, active app for the menu bar (EWMH) | `ext-foreign-toplevel-list` + `wlr-foreign-toplevel-management` (our compositor implements them) |
| desktop window (`_NET_WM_WINDOW_TYPE_DESKTOP`) | layer-shell background layer; live wallpaper as a compositor layer/shader |
| lock screen (grabs + raise loop, X11 cannot truly protect) | `ext-session-lock-v1`: nothing else can draw or get input while locked |
| `xss-lock`, idle | `ext-idle-notify-v1` / inside the compositor |
| keyboard layouts (XKB groups, xkbcomp, lesson 9/18) | the compositor owns the xkb keymap (xkbcommon) |
| clipboard / XDND | `wl_data_device`; Smithay bridges XWayland |
| monitors (`xrandr`) | compositor outputs; `wlr-output-management` |
| Launchpad background (screen capture + blur) | done in the compositor on the GPU |
| global menu (`_GTK_*` props, AppMenu registrar) | **partly lost**: GTK has no standard global menu on Wayland. Still possible for X11 apps under XWayland (the compositor sees their properties) and for KDE apps (`org_kde_kwin_appmenu`) |
| tray (SNI), notifications, Bluetooth, brightness (logind) | unchanged — D-Bus / logind |

## Phases (each ends with a go/no-go)

0. **Performance probe — first, because it decides everything.** A separate
   tiny Smithay compositor, not connected to Mimshel, running nested in a
   window (winit/X11 backend): a few real windows, a dock-like layer with the
   zoom animation, a shader wallpaper, a frame-time overlay. Measure on the
   laptop (Radeon 780M) at 60 and 120 Hz and with a 4K output; also in a VM
   without a GPU (`llvmpipe`) for the floor. **Go** if frames stay within
   ~2–3 ms on the laptop and the VM is usable.
1. **Platform boundary in today's shell**, X11 implementation only — no
   behaviour change, everything keeps passing on the laptop. Useful even if
   Wayland stops later.
2. **The real compositor, nested**: window management (focus, stacking,
   move/resize, decorations, workspaces, fullscreen), XWayland, the
   protocols above; the shell's Wayland platform implementation. Test
   Houdini, Blender, Firefox, Chrome, Telegram, Steam through it.
3. **Session on the real screen** (DRM/KMS, libinput, logind seat): an
   entry in LightDM next to Xfce; lock screen via `ext-session-lock`; crash
   handling (the compositor's own crash reports, the shell's guard).
4. **Polish on the GPU**: live wallpapers, blur, window animations.

## Phase 0 results (2026-10-26) — GO

`probes/compositor-probe` (Smithay 0.7.0, winit backend nested in the X11
session, GLES on the laptop's Radeon 780M; the panel runs at ~165 Hz, so the
real budget is ~6 ms). `render` = the compositor's own CPU+GPU work until the
GPU finished; `present` (waiting for vblank) is excluded.

| Case | render avg | p99 |
|---|---|---|
| 1280x800, vsync, wallpaper + dock | 0.69 ms | 0.95 ms |
| 1280x800, unthrottled | 0.39 ms | 0.87 ms |
| 1920x1141 (whole free screen), unthrottled | 0.66 ms | 1.2–1.6 ms |
| same, no wallpaper (dock only) | 0.31 ms | 0.71 ms |
| same, no dock (wallpaper only) | 0.61 ms | 1.15 ms |
| 1920x1141, vsync, wallpaper + dock | 0.90 ms | 1.17 ms |
| same + two kitty clients scrolling text | 0.89–0.92 ms | 2.1–2.4 ms |

- The dock zoom costs ~0.06 ms; a full-screen shader wallpaper ~0.35 ms.
- 4K could not be shown in a window on this screen; scaling the measured
  per-pixel cost gives roughly 2–2.5 ms — still inside a 120 Hz budget.
- Rare spikes (max 6–11 ms, below p99) appear while nested inside xfwm4's
  compositor; re-check on the real DRM path in phase 3.
- Not yet measured: a VM without a GPU, the NVIDIA dGPU, XWayland clients
  (xorg-xwayland is not installed on the laptop yet).

Conclusion: GPU rendering with GLES in a Smithay compositor keeps far inside
the frame budget with a live wallpaper, the dock animation and real
windows. Next: phase 1 (the platform boundary in today's shell).

## Risks to watch

- NVIDIA in the hybrid setup on Wayland (better than before, still the
  fussiest part) — test with the dGPU from phase 0 on.
- Houdini and other pro apps through XWayland — test in phase 2 before any
  switch.
- Global menu for GTK apps (see the table).
- Tooling: `xdotool`/`import` testing does not work on Wayland — the
  compositor needs its own test hooks and screenshot path early.
- Scope: this is months of work; the X11 session must stay healthy meanwhile.
