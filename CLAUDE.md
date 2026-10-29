# Mimshel — notes for AI helpers

Mimshel (Мимшел — from Saturn's moon Mimas, its Herschel crater and *shell*) is a
macOS-Sonoma-style desktop shell for Linux on **X11**, written in Rust as **one program
with one config file**. It draws the menu bar, the dock, the desktop, the file manager,
the Search and Apps windows, notifications, the Control Center, the lock screen and its
own terminal. Windows are managed by **xfwm4** (Xfce). Repository:
`github.com/ded-achtung/Mimshel`, working branch `master`.

The detailed step-by-step history up to 2026-10-28 (in Russian) is in git:
`git show cc6794d:CLAUDE.md`. This file keeps what is still true and needed.

## Talking to the user

- Answer the user **in Russian**, in plain words, without jargon: the user is not an
  expert programmer.
- **English** for everything that lands on GitHub: commit messages, PR and issue text,
  code comments (doc comments too), this file and other docs. Older Russian comments
  are translated when the code around them is touched, not in bulk.
- **UI strings** go through the language table `crates/core/src/i18n.rs`
  (`key -> (en, ru, zh, zh_TW)`, add the key to `KEYS`; a test checks every column).
  English is the default since PR #4 (2026-10-28); the Russian column keeps the original
  wording. The maintainer's laptop runs `look.language = "ru"`. Terminal output of
  `mimshel` commands (`doctor`, `set`, `apply`…) is still Russian.
- Text meant to be attached to GitHub (the README inside `mimshel doctor --report`) is
  English.

## Decisions already made

- **X11 + xfwm4.** We do not write a window manager. Target apps (Houdini and others)
  run on X11.
- **Wayland is postponed** (user, 2026-10-26). The plan — an own compositor on Smithay,
  OpenGL ES, a frame budget, XWayland — and the phase 0 result (GO) are in
  `docs/wayland-plan.md`. Resume with phase 1 only when the user says so.
- **No GTK** (GTK 5 drops X11). Own widgets (`crates/widgets`).
- **CPU rendering** with tiny-skia: works without a GPU and is the reference for
  snapshot tests. Everything is computed in **points**; `crates/render::Canvas` draws in
  pixels (`look.scale` = `auto` from `Xft.dpi`, or 1–3).
- **Own lock screen** (2026-10-23), not i3lock: see "Lock screen" below.
- **Own terminal** (2026-10-25): `alacritty_terminal` (Apache-2.0) for parsing, grid and
  PTY; window, rendering, tabs and input are ours. The default terminal for
  `Terminal=true` apps.
- **Own file manager** (2026-10-24): a Thunar window took 4.5–6.5 s to open (lesson 24);
  ours opens in 0.09–0.17 s. `parts.folders = "thunar"` brings Thunar back.
- **Own desktop** (2026-10-24) instead of xfdesktop; `parts.desktop = "xfce"` brings it
  back.
- **Tray: StatusNotifierItem only**, no XEmbed. nm-applet runs with `--indicator`.
- **The app list updates itself**: inotify on the `applications` folders, re-read after
  300 ms of quiet.
- **Look:** Colloid icons/cursor/GTK theme (GPL-3.0, built from commits pinned in
  `packaging/third-party.sh`); our overlay theme `Colloid-mimshel` (own logo — a planet
  with a ring and a bite, network icons from WhiteSur); optional pre-rendered PNG overlay
  `Colloid-mimshel-png` (`look.png_icons`); own wallpapers drawn by the program
  (`space`, `waves`, `aurora`). Some Colloid icons look like macOS ones — the user chose to
  keep them for now. No Apple artwork.
- **Names describe what a part does** (user, 2026-10-24), not macOS names:

  | Part | On screen | Code | Command | Settings |
  |---|---|---|---|---|
  | ex-Spotlight | «Поиск» / Search | `crates/launcher/src/search_panel/` | `mimshel search` | `parts.search_panel` |
  | ex-Launchpad | «Программы» / Apps | `crates/launcher/src/apps_panel/` | `mimshel apps` | `parts.apps_panel`, `[apps_panel]` |
  | search algorithm | — | `core::search` | `mimshel test-search TEXT` | — |
  | app list | — | `crates/apps` | `mimshel test-apps` | `[apps]` |
  | file manager | folder windows | `crates/folder`, `apps/mimshel/src/folder_ui.rs` | `mimshel folder DIR` | `parts.folders` |

  Old names keep working (`mimshel spotlight|launchpad`, `config::current_key`,
  `config::migrate_names`). Kept on purpose: `macos-launchpad.desktop` (the dock pins it
  under that name), `/net/launchpad/plank`, xsettings `Gtk/ShellShowsMenubar`.
- **The project was called `shell` before 2026-10-25.** All names live in
  `crates/core/src/names.rs` (`APP`, `OLD`, `CLASS`, `dir_in`, `env`/`flag`); old folders,
  variables (`SHELL_…`), desktop files, markers and config values are still recognised
  and migrated. Do not remove that compatibility without asking.
- We study how macOS and other programs behave and follow open standards (EWMH/ICCCM,
  Desktop Entry, Icon Theme, StatusNotifierItem, DBusMenu, Notifications, BlueZ,
  NetworkManager). **We never copy other projects' code** (most are GPL). Useful to look
  at for behaviour: helloSystem, Gershwin, elementary/Pantheon, COSMIC (structure of a
  Rust desktop), eww, LeftWM, penrose (x11rb usage).

## How it is built

Layers — dependencies only go downwards:

| Layer | Crates |
|---|---|
| Model and settings | `core` (config, search, i18n, crash report text, names) |
| Platform | `x11` (x11rb), `render` (tiny-skia), `text` (cosmic-text), `icons` (resvg, PNG/JPEG), `apps` (Desktop Entry), `dbus` (zbus 5, blocking) |
| Shell parts | `launcher`, `dock`, `menubar`, `notify`, `desktop`, `folder`, `terminal`, `lock`, `wallpaper`, `widgets` |
| Setup and diagnostics | `setup` (`set`, `apply`, `doctor`, `backup`/`restore`, greeter, lock guard) |
| The program | `apps/mimshel` (the `*_ui.rs` files glue parts to X windows) |

Known layering debts (fix when touching): `core::names` does file renames and reads
the environment; the service links the whole `setup` crate.

### Processes

- `mimshel daemon` is the **guard** (`apps/mimshel/src/guard.rs`): it runs the real
  service `mimshel daemon --child` and restarts it. Rules (`next_step`): a crash →
  restart after 1, 2, 3, 4 s; five quick crashes in a row → a dialog and give up; a run of
  60 s starts the count anew; TERM/HUP (a deploy, the session ending) restart at once and
  clear the count. The guard exits when X closes (`ServerWatch`, lesson 17); the service
  exits when the guard is gone. Log: `~/.local/state/mimshel/daemon.log` (kept under
  1 MB, the previous one is `daemon.log.1`).
- The service holds the menu bar, dock, desktop, folder windows, Search/Apps, notifications
  and the tray/menu D-Bus services. **Any panic in the service ends the whole process**
  (the panic hook exits with 101) so the guard restarts it; a panic that is handled on
  purpose must go through `mimshel_core::crash::catch`.
- Separate processes: `mimshel terminal`, `mimshel lock-screen` (under the `mimshel lock`
  guard). Commands reach the service through `$XDG_RUNTIME_DIR/mimshel.sock` (folder 0700,
  owned by the user).
- Crash reports: `~/.local/state/mimshel/crashes/`, `mimshel crashes [last]`; the
  restarted service shows one notification from `last-crash`.

### Settings and files

- `~/.config/mimshel/config.toml` (`MIMSHEL_CONFIG` for another file): edited with
  toml_edit (comments survive), checked for syntax, key names, types and ranges. A broken
  file never replaces the last good copy `~/.local/state/mimshel/config.good.toml`; the
  service falls back to it. Writes take `config.toml.lock` and replace the file
  atomically.
- `mimshel set SECTION.KEY VALUE` writes and applies; `--no-apply` only writes;
  `mimshel config` lists settings; `mimshel apply [parts…]` (menubar dock notify desktop
  folders launchers lock power icons look wallpaper service; `greeter` separately, asks
  for sudo once). **Every `apply` restarts the service.** Hot reload without a restart
  exists only for `look.language` and `look.wallpaper` (`mimshel reload`).
- **System Settings window** `mimshel settings [PAGE]` (`crates/settings` without X11,
  `apps/mimshel/src/settings_ui.rs`): its own process, so it survives the service
  restarts its applies cause. It writes and applies through the same code as `mimshel set`
  (`write_setting` / `apply_setting_checked`), one change at a time on a worker thread.
  System pages open the Xfce dialogs. The logo menu and the dock's settings icon open it
  (`mimshel-settings.desktop`, written by `apply launchers`). A frame redraws only the half
  that changed and sends only the changed rectangle (`MIMSHEL_TIMING=1` prints frame times).
- `~/.local/share/mimshel`: `apple.menu` + `menu/` (main menu), `backups/` (`mimshel
  backup|backups|restore [--dry-run]`; the first copy, «original», is plain Xfce),
  `wallpapers/`. `~/.local/state/mimshel`: logs, `desktop-icons` (icon cells),
  crashes. `~/.cache/mimshel/lock/`: lock backgrounds.
- `mimshel doctor [--fix]` checks the setup; `doctor --report` makes an archive for a bug
  report with home path, user names, real name, host and Wi-Fi names removed.

### Screen and windows

- The menu bar, the dock, notifications and Search live on the **primary monitor**
  (`Display::primary_area`: RandR primary, else the first monitor). The menu bar's strut
  reserves only that monitor's columns. The dock reserves no space, even with
  `dock.hide = never`: maximized windows take the whole height under it (user,
  2026-10-29). RandR changes arrive as `Event::Configure` of the root. The
  desktop draws wallpapers per monitor; desktop icons are on the first monitor (to be
  decided after user tests); the Apps window covers the whole screen.
- Menu bar icons (tray, layout, volume, battery, bell — not the Control Center icon or the
  clock) move into the Control Center by dragging them down out of the bar and back by
  dragging them up from its «Hidden icons» row; `[menubar] hidden` keeps their names
  (`tray:ID` uses the item's StatusNotifierItem `Id`). These icons act on release, not on
  press, so a drag does not also click them. A drop anywhere below the bar or onto the
  Control Center icon hides it. Hidden nm-applet/blueman icons stay in that row even though
  the Control Center has Wi-Fi and Bluetooth tiles (user, 2026-10-29).
- Big images go through MIT-SHM (`crates/x11/src/shm.rs`, no `unsafe`, falls back to
  PutImage; `MIMSHEL_NO_SHM=1` turns it off).
- The dock matches windows to apps by `GIO_LAUNCHED_DESKTOP_FILE`/pid environment,
  `StartupWMClass`, desktop file name, command name (lesson 6), and re-reads only new or
  changed windows.
- Global menu: GTK (`org.gtk.Menus`/`Actions`) and DBusMenu through our own
  `com.canonical.AppMenu.Registrar`. GTK apps hide their in-window menu only while a
  registrar is on the bus. D-Bus calls to other programs time out after 3 s
  (`mimshel_dbus::session()`).
- Drag and drop: XDND in `crates/x11/src/dnd.rs`, both directions (desktop ↔ folder
  windows ↔ other programs). Same disk — move, other disk or Ctrl — copy.

### Lock screen

`mimshel lock-screen` (`apps/mimshel/src/lock_ui.rs`, background in `crates/lock`):
full-screen window with an **opaque server-side background** (lesson 20), keyboard and
pointer grab counted only on `SUCCESS` (lesson 21), raised on every stacking change and
every 0.4 s, re-grabs if the grab is lost (never leaves), redraws on a screen size change.
Password: PAM `system-local-login`, in a `Zeroizing` buffer allocated up front; PAM's own
messages (locked account, expired password) are shown. Guard `mimshel lock`
(`crates/setup/src/lock.rs`): restarts the screen on any non-zero exit, one guard per
user and display (abstract socket, lesson 27); a guard that loses waits until the screen
announces it is covered before letting the system sleep. Super+L and Ctrl+Alt+L →
xflock4 → `LockCommand` → `mimshel lock`; xss-lock (`--transfer-sleep-lock`) locks on idle
and before sleep. Not done: closing xss-lock's sleep fd early (needs `unsafe`), so sleep
waits up to logind's 5 s. **Any change in the lock path needs a crash test** (lesson 1).

## Building and testing

Rust 1.99.1 (`RUSTUP_TOOLCHAIN` in CI). The same checks as CI
(`.github/workflows/check.yml`):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p mimshel-x11 --test windows --locked -- --ignored   # needs Xvfb
cargo deny --locked check                                           # deny.toml
```

- X11 tests start their own Xvfb (`MIMSHEL_TEST_XVFB` — its path) and never touch the
  working display: windows, pixels, struts, two monitors, drag and drop from another
  program. D-Bus tests run on a private bus (`dbus-run-session`). Rendering tests need
  system fonts (`fonts-dejavu-core`). Terminal tests run a real shell.
- Pictures without a screen: `mimshel snapshot search|apps|lock|folder … FILE.png`.
- Helpers: `cargo run -p mimshel-dbus --example tray|appmenu|bluetooth`,
  `-p mimshel-x11 --example windows|attrs|xsettings|xkb_group`, `mimshel test-search`,
  `test-apps`, `test-clipboard`, `watch`.
- Debug switches (also with the old `SHELL_` prefix): `MIMSHEL_TIMING=1` (per-step and
  slow-frame timings), `MIMSHEL_DEBUG_DOCK=1`, `MIMSHEL_DEBUG_DESKTOP=1`,
  `MIMSHEL_DEBUG_KEYS=1` (never reaches the lock screen), `MIMSHEL_SCALE`.
- A git worktree builds into its **own** `target` (lesson 19).
- Before saying "done": tests green **and**, for anything visible, a look at the real
  screen (lesson 8).

## Machines and deploying

- **Laptop** (the user's real machine, EndeavourOS + Xfce 4.20 on X11, us/ru layouts,
  terminal kitty): Mimshel runs there daily, installed by `./install.sh`. This is where
  changes are checked live.
- **Deploy a new build without moving the service out of the session** (lessons 3, 17):
  `install -m 755 target/release/mimshel ~/.local/bin/mimshel`, then `kill -TERM` only the
  `mimshel daemon --child` process (find it by full command line, lesson 13) — the guard
  restarts it from the new file. **Never run `mimshel set` without `--no-apply` or any
  `mimshel apply …` over SSH**: every apply restarts the service, and it ends up in the
  SSH session. Write with `set --no-apply` and ask the user to run `mimshel apply …` in a
  terminal on the laptop.
- To run something in the graphical session from SSH, take the whole environment of
  `xfce4-session` from `/proc/PID/environ`, including `LANG`/`LC_*` (lesson 14).
- Screenshots: `DISPLAY=:0 import -window root /tmp/x.png`. The user often works at that
  desktop: **ask before clicking or typing into it**. `xdotool type` only with Latin text
  (lesson 9).
- `sudo` needs the user's password — ask, never guess. Packages: `pacman -Syu` (never a
  bare `-Sy`); AUR by hand with `makepkg`.
- `install.sh`: packages, Colloid themes from pinned commits, `cargo build --release
  --locked`, a backup, `mimshel set --no-apply …`, `mimshel apply`, `doctor`. Safe to run
  again. `offline/` (themes + vendored crates) is an untracked folder on the laptop — keep
  it.
- Early development happened in a VirtualBox VM (no GPU, llvmpipe, clock jumps —
  lesson 23); numbers measured there are marked as such.

## Measurements

Memory is PSS (`/proc/PID/smaps_rollup`), CPU from `/proc/PID/stat` ticks.

| (VM, 1278×798, 2026-10-22) | Memory | Idle CPU |
|---|---|---|
| Guard + service (menu bar, dock, tray, Search, Apps) + `pactl`/`xfconf-query` | 15.6 MB | 0.1 % |
| Before: Xfce panel with 6 plugins + Plank (+ rofi per open) | 141 MB | — |

Also measured in the VM: Search opens in 0.1–0.2 s (rofi took ~1 s), a folder window in
0.09–0.17 s, a wallpaper change takes ~75 ms. The guard sets `MALLOC_MMAP_THRESHOLD_`/`MALLOC_ARENA_MAX` so
memory returns to the base after big pictures. Measure the same way to compare.

## Lessons (learned the hard way — do not repeat)

The numbers are referenced from code comments; keep them.

1. **A lock screen that crashes on X11 unlocks the screen.** Anything in the lock process
   only after a crash test (`kill -ABRT`, lesson 13); always a guard that restarts it.
2. **Process names are cut to 15 characters** in the kernel: `pgrep -x xfce4-screensaver`
   finds nothing. Match the command line: `pgrep -f '^(\S*/)?NAME(\s|$)'`.
3. A process started over SSH lands in the **SSH logind session**: no `lock-session`
   from the desktop, no session language. Important daemons start from the session
   (autostart).
4. **Never enable GTK modules on the fly** through xsettings `/Gtk/Modules` — running GTK
   apps crash. Only through `GTK_MODULES` at login.
5. An icon theme may have different pictures for one name in different folders
   (WhiteSur: `org.xfce.thunar` is Finder in apps/scalable and a folder in
   preferences/32). Choose deliberately.
6. **Plank tied windows to the desktop file the app was launched with**; copies of desktop
   files elsewhere gave duplicate icons. Our dock matches by WM_CLASS, _NET_WM_PID,
   GIO_LAUNCHED_DESKTOP_FILE and StartupWMClass.
7. i3lock-color does not do arithmetic in coordinates (`h*0.12+121`) — only numbers.
8. Before saying "done": look at the screen (a screenshot) and run `mimshel doctor`.
9. **`xdotool type` with Cyrillic breaks the keyboard layouts** (it remaps keys the old
   way, XKB loses the second layout). Restore: `setxkbmap -rules evdev -model pc105
   -layout us,ru -option '' -option grp:alt_shift_toggle`. For Russian input in tests:
   the `xkb_group` example (switch group, press keycodes).
10. `pkill -f 'PATTERN'` also matches the shell running that command — and kills it. Stop
    by pid: `kill $(pgrep -f '^target/release/mimshel daemon')`.
11. **Selecting events on another client's window replaces our whole event mask on it.**
    Our own dock window is in `_NET_CLIENT_LIST`; subscribing to "all app windows" wiped
    its pointer events. Skip own windows.
12. The service draws the dock: if it dies there is no dock until it is back. **Every
    panic is a bug**; the guard restarts the service (see "Processes").
13. **Test a crash with `kill -ABRT`, not `-SEGV`**: Rust handles SIGSEGV (stack overflow
    guard) and survives a sent one. Find the process by its full path
    (`pgrep -f '^/home/USER/.local/bin/mimshel daemon --child'`), see lesson 10.
14. **Start the service by hand only with the full session environment**: without
    `XDG_CURRENT_DESKTOP` the `OnlyShowIn=XFCE` launchers vanish (the Settings icon left
    the dock).
15. **`xfce4-panel --quit` reset the layouts to `us` only** (not every time). Check:
    `xkbcomp -xkb $DISPLAY - | grep xkb_symbols` (want `us+ru:2`); restore with lesson 9's
    command.
16. Lesson 11 again: subscribing to app windows turned off clicks on the menu bar.
    `watch_window` now **adds** to the existing mask (`your_event_mask`).
17. **A service restarted over SSH lives outside the graphical session** (lesson 3) and
    survived logout; the guard kept restarting it until it attached to the next
    session's X. Now the guard holds a quiet X connection (`ServerWatch`) and exits when X
    closes.
18. **The shared X layout follows the keyboard of the last key press.** The XTEST virtual
    keyboard (xdotool) starts with `us` and one synthetic press switches everything to
    `us`. `doctor` looks at real keyboards (`xkbcomp -i ID`); the menu bar listens for XKB
    new-keyboard events; the service gives XTEST all layouts at start
    (`fix_virtual_keyboard`).
19. **Do not build a second checkout (git worktree) into a shared `target`**: cargo took
    the other tree's build as fresh — a branch build got installed and tests looked for
    files in a deleted folder. Each checkout its own `target`.
20. **A window with transparency is transparent until the program's first frame.** The
    lock screen "covered" the screen but for 2–3 s the desktop and notifications showed
    through. Windows that must cover use an opaque `background_pixel`: the server paints
    them the moment they map.
21. **`GrabStatus::ALREADY_GRABBED` means ANOTHER client holds the grab**, not us (our
    own repeated grab returns `SUCCESS`). Counting it as success means keys go elsewhere.
22. **A disabled autostart does not mean "will not start": D-Bus activates a program on
    the first call to its name** (`/usr/share/dbus-1/services`). The Xfce screensaver
    locked the laptop with its own window, bypassing `LockCommand`. Mask it with a file of
    the same name in `~/.local/share/dbus-1/services` (`Exec=/bin/false`) and ask the bus to
    reload (`org.freedesktop.DBus.ReloadConfig`; dbus-broker does not rescan by itself).
    xfce4-notifyd is masked through systemd.
23. **VM clock jumps** (VirtualBox time sync): files built "in the future" look newer than
    edited sources and cargo skips rebuilding them. Fix: `find crates apps -name '*.rs' |
    xargs touch`. Sign: `target/release/mimshel` newer than `date`.
24. **gdk-pixbuf 2.44 (Arch) loads images through glycin: ~65–80 ms per SVG**, ~7 ms per
    PNG. Pre-rendered PNG icons helped GTK apps only 10–40 %; the rest is GTK itself. PNG
    data inside `icon-theme.cache` is worse (367 MB). Do not time Thunar under
    `dbus-run-session` (it waits for volume monitors).
25. **Thunar runs as a daemon (`thunar --daemon`)**: closing windows does not stop it; it
    sees new `gtk.css` rules only after `thunar -q`. Restart it with the full session
    environment (lesson 14).
26. **`mimshel apply service` could leave no service**: the old one had 3 s to exit, the
    new one saw "already running" and left, then the old one exited. Now `sys::kill`
    sends `-KILL` after 3 s.
27. **Before sleep two programs ask to lock at once** — xss-lock and the Xfce power
    manager (via xflock4). A process-list check let both through: two lock screens, the
    password asked twice. One guard per user and display through an abstract socket
    (`lock::single_guard`); the kernel frees it when the process ends.
28. **Colloid-Dark and Colloid contain relative symlinks into Colloid-Light** (mimetypes,
    apps…). Deleting Colloid-Light as "unused" broke the dock's Apps icon (2026-10-26).
    Before deleting any theme, check for broken links with `find -L THEMES -type l`. Twelve
    broken `search.svg` links are an upstream Colloid bug.

## Rules of work

- Small meaningful commits with English messages; `cargo fmt` and `cargo clippy -D
  warnings` clean; `unsafe` is denied for the whole workspace.
- Do not touch system files unless needed; everything in the home folder, with a way
  back (`mimshel restore`, markers that `restore` removes).
- Nothing is deleted and no machine is rebooted without the user's consent.
- Plans and status: `NEXT.md` (next steps; scope and non-goals only after three real user
  tests), `CHANGELOG.md` (update before any tag), `docs/roadmap-review.md`,
  `docs/wayland-plan.md`, `docs/i18n.md`.
