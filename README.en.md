# shell (working title)

[Русский](README.md) · **English**

A macOS-style desktop shell for Linux/X11, written in Rust. Windows are managed
by xfwm4; shell draws the dock, the menu bar, Spotlight, Launchpad and
notifications. Rendering is done on the CPU with tiny-skia, text with
cosmic-text, X11 through x11rb and D-Bus through zbus. Wayland is not supported.

## Features

- Spotlight with application search, including when typed in the wrong keyboard
  layout, and Launchpad.
- Dock: pinning and reordering icons, window grouping, window previews, Trash.
- Menu bar: global menu (GTK and DBusMenu), StatusNotifierItem tray, keyboard
  layout, clock with a calendar, volume, battery and Control Center. The legacy XEmbed tray is
  not implemented.
- Notification banners and history (click the bell), Do Not Disturb.
- TOML settings, `doctor` diagnostics, switching each part between shell and the
  original Xfce components.
- A watchdog for the service, and i3lock-color started under a lock watchdog;
  the lock screen background is drawn by shell itself.
- Settings backups and restoring the original Xfce setup (`backup`, `restore`).

For now the project expects a configured Xfce session and the `macos-look`
environment. Installing packages, themes, icons and the cursor, the wallpaper,
and hiding or renaming applications are still done by it. This is not a
standalone desktop installer.

## Installing

You need EndeavourOS or Arch Linux with the Xfce desktop (an X11 session). In an
Xfce terminal:

```sh
git clone https://github.com/ded-achtung/shell.git
cd shell && ./install.sh
```

The installer asks for your sudo password. It installs the packages, builds the
i3lock-color lock screen with our patch (`packaging/`), installs the Colloid icons,
window theme and cursor, builds shell into `~/.local/bin/shell` and turns all of its
parts on. It saves a copy of your settings first: `~/.local/bin/shell restore`
brings them back. Log out and back in afterwards. Running it again is safe.

## Building and testing

Tested with Rust 1.99.1. The rendering tests need system fonts (for example the
`fonts-dejavu-core` package). Unit tests do not need an X server. The
notification integration test starts its own bus with `dbus-run-session`
(package `dbus`).

```sh
cargo build --release --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
# Separately: windows on a virtual X server (needs Xvfb)
cargo test -p shell-x11 --test windows --locked -- --ignored
```

The same checks run in GitHub Actions. The X11 test starts its own Xvfb and
checks showing, hiding and resizing windows, window pixels and the panel's
reserved screen space, both with and without MIT-SHM. `SHELL_TEST_XVFB` sets the
path to Xvfb if it is not in PATH. The D-Bus test checks notifications, actions,
closing, and reconnecting after the service restarts.

A normal test run skips the Xvfb test (run it with the separate command above)
and the Launchpad background speed benchmark. The tests do not cover a real
screen lock, Wi-Fi/Bluetooth hardware, or compatibility with every application.

Running shell needs an X11 session with D-Bus; it uses `pactl`,
`xfconf-query`, `nmcli`, `bluetoothctl` and GNU coreutils, including `timeout`.
The battery is read straight from the kernel files in `/sys/class/power_supply`
— `upower` is not needed. Locking needs i3lock-color and xss-lock. The font and
icon theme are set in the config; the defaults are Inter and WhiteSur-dark-mac
from the macos-look setup.

## Running and settings

```sh
./target/release/shell apps
./target/release/shell search terminal
./target/release/shell daemon
```

The last command starts the service in the current graphical session. From
another terminal, `shell spotlight` and `shell launchpad` open those windows.
Commands go through `$XDG_RUNTIME_DIR/shell.sock`. If that variable is not set,
`/run/user/UID/shell.sock` is used, and if that directory does not exist,
`/tmp/shell-runtime-UID/shell.sock`. The directory must belong to the user and
be closed to everyone else (0700); the fallback directory is created that way.
Run the program with `--help` for the full list of commands.

Settings live in `$XDG_CONFIG_HOME/shell/config.toml` or
`~/.config/shell/config.toml`. On first start they are carried over from
`macos-look`. `SHELL_CONFIG` points to a separate file for testing.
`shell config` lists the available keys; `shell set KEY VALUE` saves a setting
and applies it, restarting the service if needed. `shell doctor` checks the
environment, and `shell doctor --fix` changes it to fix the problems it finds.

The file is checked for syntax, key names, types and value ranges. Icon sizes:
16–256, menu bar height: 16–128, dock magnification: 100–300 %, Launchpad grid:
1–32 rows and columns. A broken file never replaces the last good copy,
`$XDG_STATE_HOME/shell/config.good.toml` (by default
`~/.local/state/shell/config.good.toml`). The service falls back to that copy on
a syntax, value or read error; a deleted main file is also restored from it. An
existing file that is damaged or unreadable is never overwritten automatically.

Changes from the dock and from the command line are made under a shared
cross-process lock, `config.toml.lock`: reading, editing, replacing the file and
updating the backup copy happen one after another. Temporary files get unique
names and are created with 0600 permissions. This protects concurrent changes
made by shell itself; other text editors do not use this lock.

The lock watchdog keeps retrying after repeated i3lock failures for as long as
the X server check confirms that the session is alive. The first retry happens
immediately, later ones wait up to a second so a constant error does not spin.
On X11, restarting a crashed locker **does not guarantee the screen stays
locked**: the screen can be reachable until the new locker grabs input.

## Structure

- `core` — model, search, settings parsing; no windows and no file I/O.
- `x11`, `text`, `icons`, `apps`, `dbus` — platform functions.
- `widgets` — shared menus and drawing helpers.
- `launcher`, `dock`, `menubar`, `notify` — logic and drawing of the shell parts.
- `lock` — the lock screen background.
- `setup` — diagnostics, applying settings, session actions, the lock watchdog,
  backups and restore.
- `apps/shell` — commands, windows, background watchers and event handling.

## Approach

We study how existing programs behave and the open standards (EWMH/ICCCM,
Desktop Entry, Icon Theme, StatusNotifierItem, DBusMenu, Notifications), and
write our own implementation. We do not copy other projects' code. Passwords are
checked by i3lock-color; shell does not implement its own authentication.

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
