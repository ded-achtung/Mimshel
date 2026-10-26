# Roadmap review — what to do, what to postpone, what to drop

**Date:** 2026-10-25. **Input:** an external list of ten improvement areas
(threads, failure visibility, logging, UI file split, accessibility,
localization, hardware matrix, render budgets, installer, scope), checked
against the code at `audit-fixes` (5e1e5dd) and against `NEXT.md`.

This is a working note, not a scope decision: `NEXT.md` says scope and
non-goals are fixed only after the three user tests. Everything below is a
hypothesis to revisit after them.

## Facts it is based on

- 52 `thread::spawn` call sites in `apps/` and `crates/`, 34 `thread::sleep`
  call sites, 12 direct `.spawn()` of child processes (some were not reaped —
  audit item 6, being fixed on `kimi/audit-limits`).
- Largest files: `crates/folder/src/view.rs` 1791 lines,
  `menubar_ui.rs` 1507, `desktop_ui.rs` 1373, `term_ui.rs` 1280,
  `ui.rs` 1050, `folder_ui.rs` 1033, `dock_ui.rs` 1021.
  `crates/folder` is already X-free (`FolderView` draws to a pixmap), so the
  biggest file is testable without an X server today.
- The service guard gives up after 5 fast crashes (CLAUDE.md, lesson 12):
  after that there is no dock / menu bar until the next login, and nothing
  tells the user why.
- `mimshel doctor` already exists; `MIMSHEL_TIMING=1` already logs per-step
  timings; logs are plain text in `~/.local/state/mimshel/`.
- UI strings are Russian **by the user's decision** (2026-10-25); comments,
  commits and GitHub text are English.

## Verdict per area

| # | Area | Verdict | Why |
|---|---|---|---|
| 1 | Threads, timers, processes | **Do the cheap part now, postpone the rest** | One shared helper for detached child processes (in progress). Name long-lived threads and show thread / child / fd counts in `doctor`. A timer scheduler or worker pool only if those counts show a real leak — std threads are cheap and there is no measured problem. |
| 2 | Visible failures | **Do first** — highest value | See "Safe failure" below. Extend `doctor` instead of adding a separate `status` command. Restarting a single component is not possible while all parts live in one process — drop. |
| 3 | Logs and metrics | **Postpone structured logs; do a manual benchmark** | JSON/structured logging is overhead for one developer. Instead: a repeatable `tools/bench.sh` run on the laptop (cold start, search / folder open, RSS, idle CPU) with results kept in `docs/`. CI timing on shared runners under Xvfb is too noisy to catch regressions — drop that part. |
| 4 | Split big UI files | **Opportunistically, not as a project** | Split a file when a feature touches it (state / layout / input / effects). A dedicated refactor now would conflict with feature branches and carries risk without a user-visible gain. |
| 5 | Accessibility (AT-SPI) | **Postpone; keep cheap parts** | A full AT-SPI layer (e.g. AccessKit) is a large project. Keep doing keyboard navigation and visible focus; `look.scale` already covers text size; add a "reduce motion" switch when animations grow. Revisit if a tester needs a screen reader. |
| 6 | Localization (Fluent) | **Postpone until a non-Russian tester** | Contradicts the current decision (Russian UI). If one of the three testers does not read Russian, introduce Fluent then, starting with the menu bar and folder windows. |
| 7 | Hardware matrix | **Do** — overlaps `NEXT.md` §2–4 | A written checklist run on the laptop (below). A VM covers install / re-install / restore / lock crash; suspend, lid, NVIDIA, hot-plug and real Bluetooth / Wi-Fi need the real machine. |
| 8 | Render budgets | **Measure first, as already planned** | Check: the clock tick must not redraw the whole menu bar; icon / wallpaper / font caches need upper bounds; 4K and two monitors. No GPU rendering until profiling shows rasterization is the bottleneck (agrees with the existing decision). |
| 9 | Installer as a product | **Partly** | Useful: `install.sh --dry-run` listing every change; `mimshel uninstall` (restore + remove binary, data, autostart); later an AUR `PKGBUILD`. Drop for now: SBOM and release signing — too much for an alpha with three testers (checksums for the offline archive already exist). |
| 10 | Project boundaries | **Agree with the order, not with a document now** | Working priority: service + lock screen → dock / menu bar / search → file manager → terminal → extras. No Wayland, plugins or new panels. Write `SCOPE.md` / `NON-GOALS.md` only after the user tests, as `NEXT.md` requires. |

## Safe failure (the gap the list missed)

The most important missing piece is not a log format but what the user sees
when Mimshel fails:

1. **Guard gives up → fall back to Xfce.** When the guard stops after 5 fast
   crashes, start the previous programs for the parts Mimshel was doing
   (xfce4-panel, Plank, xfdesktop, Thunar for folders) and show a plain
   message (zenity / xmessage — our own notification service is down at that
   moment). Today the desktop just loses its panels.
2. **Lock screen cannot start.** Verify on the laptop what happens before
   suspend if `mimshel lock-screen` fails every time (PAM missing, X error):
   the machine must not go to sleep with an unlocked session silently. If it
   cannot lock, say so on screen.
3. **`mimshel doctor --report`** — one archive for testers: doctor output,
   last N lines of `daemon.log` / `lock.log`, versions, monitor layout;
   user name, home paths, notification text and clipboard removed. This is
   what the three user tests need to send back.

## Hardware checklist (laptop, per release)

- Suspend / resume and lid close: locked before sleep, unlock works, dock and
  menu bar survive.
- Second monitor: plug / unplug while running, different scale, negative
  coordinates.
- Scale 100 / 125 / 150 / 200 %.
- Keyboard layout switch while typing the lock-screen password.
- No battery / Bluetooth / Wi-Fi present (desktop PC), PipeWire restart,
  NetworkManager restart.
- Hybrid NVIDIA: launching on the dGPU.
- File manager: 1000+ files, symlink loops, USB stick, phone (MTP).
- Install on a clean user, re-install, `restore`, interrupted restore.

## Suggested order

1. Finish the audit fixes (`audit-fixes` + Kimi's `kimi/audit-limits`), test
   on the laptop, merge.
2. Safe failure 1–2 (Xfce fallback, lock-screen failure on suspend).
3. `doctor --report` + thread / process counts in `doctor`.
4. Run the hardware checklist; fix what breaks.
5. `install.sh --dry-run`, `mimshel uninstall`.
6. The three user tests (`NEXT.md` §1) — then decide localization,
   accessibility and scope with real feedback.

Ongoing: split big UI files only when a feature touches them; benchmark
script before any performance claim.

## Status (2026-10-26)

- **1 — done.** Audit fixes merged and tested on the laptop.
- **2 — partly done.** Crash reports, plain-Russian reasons and the restart
  banner are in (`mimshel crashes`, `doctor`); lock screen before suspend
  checked live (it locks first; a dead built-in keyboard after s2idle was
  a kernel issue, fixed on the laptop by a keyboard re-probe service).
  **Postponed** by the user's decision: the automatic fallback to the Xfce
  panel/Plank when the guard gives up — the give-up dialog already says
  `mimshel restore`.
- **3 — postponed** (`doctor --report`): there is one user for now; return
  to it before the three user tests.
- Also done that day: brightness slider, login screen styling
  (`mimshel apply greeter`), terminal menu surviving service restarts,
  phones in the file manager.
