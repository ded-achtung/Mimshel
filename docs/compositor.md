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
| 2.1 | Skeleton: winit and headless backends, xdg-shell windows, keyboard and pointer, focus on click, stacking, moving (Super + drag and the client's own move request), resizing (client request), damage-tracked rendering, frame timing. kitty runs inside. | Codex |
| 2.2 | Window decorations of our own (server side, `xdg-decoration`): title bar with the three buttons on the left, as on X11 today; maximize and minimize, fullscreen. | — |
| 2.3 | Protocols for the shell: `wlr-layer-shell`, `ext-foreign-toplevel-list` + `wlr-foreign-toplevel-management`, `xdg-activation`, `ext-session-lock-v1`, `ext-idle-notify-v1`. | — |
| 2.4 | The door's Wayland backend (`platform.rs` gets the second variant): the menu bar, the dock and the desktop run inside the nested compositor. | — |
| 2.5 | XWayland: Houdini, Blender, Chrome, Telegram, Steam inside (needs `xorg-xwayland` on the laptop). | — |
| 2.6 | Clipboard and drag and drop between programs and our windows; keyboard layout switching. | — |
