# Changelog

Kept since the first public alpha. **Mimshel** became the project name after the first alpha tag, which still used the working name `shell`. Entries before v0.0.2 are in Russian.

## [Unreleased]

## [v0.0.3-alpha] — 2026-10-26

Third alpha: a security audit worked through, crash reports, and a few things people asked for.
Tested on the same laptop (Tanshi X669, EndeavourOS, Xfce 4.20, X11).

### New

- **Crash reports.** Any part of Mimshel that fails leaves a report in
  `~/.local/state/mimshel/crashes/`: for a panic the message, place and backtrace; for a signal
  (SIGSEGV, SIGABRT, the OOM killer's SIGKILL) the reason and the last 30 log lines. The service
  comes back by itself and shows «Mimshel перезапустился после сбоя» with the reason; after
  5 quick crashes a dialog explains it and how to go back to Xfce. `mimshel crashes [last]`
  lists them, `mimshel doctor` reports crashes of the last week. Normal restarts (an update,
  `mimshel apply`) are not counted as crashes.
- **`mimshel doctor --report`** — one archive for a bug report: system info, all `doctor`
  checks, the config, the last 500 log lines and the crash reports, with the home path, user
  name, host name and Wi-Fi names removed. Attach it to a GitHub issue after looking inside.
- **Brightness slider** «Дисплей» in the Control Center, through systemd-logind — no root.
- **Login screen styling:** `mimshel apply greeter` gives LightDM's slick-greeter Mimshel's
  wallpaper, theme, cursor and font (asks for the sudo password once; `mimshel restore` undoes it).
- **Phones and cameras** (MTP/PTP) in the file manager's Devices.
- Terminal: `terminal.osc52 = "allow" | "off"` controls whether programs may set the clipboard.

### Security (from an external audit)

- The installer builds the Colloid themes from **pinned, reviewed commits** and checks them;
  the offline archive carries the same pins and a SHA-256 file.
- **Restoring settings is all or nothing:** a failure halfway no longer leaves half-deleted
  Xfce/Plank settings.
- The lock screen keeps the password in **one zeroizing buffer** and no longer leaves a copy in
  the PAM library's conversation.
- Notifications are bounded (text sizes, at most 8 actions, images downscaled, at most 20 per
  second); OSC 52 clipboard writes are limited to 1 MiB.
- CI: actions pinned by SHA (`actions/checkout` v7.0.1), dependency audit with `cargo-deny`
  (RustSec advisories, licenses, sources), Dependabot.
- README: what an X11 lock screen can and cannot protect against.

### Fixed

- The terminal's menu in the top bar no longer disappears when the service restarts.
- The installer ignores a stale `offline/vendor` instead of failing the build.
- Detached programs started from the terminal no longer stay as zombies.

### Changed

- With Mimshel's own file manager, the Thunar daemon is no longer started at login (~130 MB);
  Thunar still starts on demand (e.g. the dock's «empty trash»).

## [v0.0.2-alpha] — 2026-10-25

Second alpha. Tested daily on a real laptop (Tanshi X669: Ryzen 9 7940H, Radeon 780M + RTX 4070,
EndeavourOS, Xfce 4.20, X11). From this release on, commit messages, code comments and the GitHub
pages are in English.

### New

- **Own terminal** `mimshel terminal` — now the default terminal. Core: `alacritty_terminal`
  (Apache-2.0); window, rendering, tabs and input are Mimshel's own. Tabs (Ctrl+Shift+T/W,
  Ctrl+PageUp/PageDown, Alt+1…9), an app menu in the top menu bar, colour emoji and CJK via
  fallback fonts, mouse for htop/mc, OSC 52 clipboard (copy from programs over SSH), Ctrl+click
  links, a scrollbar and scrollback keys for touchpads, Ctrl+wheel zoom, blinking cursor,
  `-e PROGRAM` for apps with `Terminal=true`. Set up by `apply launchers` and `apply look`
  (Ctrl+Alt+T, Thunar «open terminal here»).
- **Wi-Fi panel** — nearby networks with signal and lock, connect with one click (NetworkManager).
- **Bluetooth panel** — paired and nearby devices, pair and connect (BlueZ over D-Bus).
- **Per-app volume** in the sound panel, like the Windows volume mixer.
- **Discrete GPU launching** — apps that ask for it (`PrefersNonDefaultGPU`, e.g. Blender) start
  on the NVIDIA GPU in hybrid mode; a dock menu item does it for any app.
- **About this computer** — Mimshel's own window (the planet logo, all GPUs named properly).
- **Window menus for own apps** — `crates/dbus/src/menu_export.rs` publishes DBusMenu menus.
- File manager looks like Thunar: path bar, search, places and devices, status bar.
- Lock screen on **Super+L**.

### Fixed

- Security: a `.desktop` file without the executable bit is no longer shown as a document and
  asks before running (a downloaded «invoice.desktop» could pose as a PDF).
- Lock screen: the user name comes from the system (`id -un`), not `$USER`; before suspend only
  one lock screen starts (the password was asked twice).
- Menu bar popups close on a click elsewhere or when another window becomes active (the
  calendar kept the keyboard).
- Alt+Tab shows the choice in the switcher instead of a window outline.
- Dock: entries like `org.telegram.desktop` are no longer cut to `org.telegram`.
- `doctor`: the laptop lid switch is not reported as a keyboard without layouts.
- CI: the X11 window test no longer fails at random.

### Changed

- The project is named **Mimshel** everywhere (command `mimshel`, `~/.local/bin/shell` stays a link).
- README is in English with a banner and a screenshot; the Russian one is `README.ru.md`.

## [v0.0.1-alpha] — 2026-10-25

**Первый документированный альфа-срез.** Версия предназначена для знакомства с прототипом и сбора вопросов пользователей, а не для обещания стабильности. Тег относится к экспериментальной ветке `fix/master-audit-20261024`; в `master` эти изменения ещё не объединены. Номер версии Rust workspace (`0.1.0`) пока не меняется.

### Что уже есть

- Оболочка Linux/X11 на Rust поверх `xfwm4`: рабочий стол, Dock, верхняя панель и глобальное меню.
- Окна поиска программ, списка программ и папок; значки рабочего стола, выбор обоев и настройки.
- Уведомления, системные индикаторы и взаимодействие через X11/D-Bus.
- Диагностика окружения, резервное копирование настроек и возможность отката к компонентам Xfce.

### Изменения в экспериментальной ветке

- Уточнены проверки переноса и копирования файлов, включая конфликты назначения; значки не скрываются заранее до подтверждённых событий файловой системы.
- Ускорена расстановка значков с использованием множества занятых клеток и единым пересчётом группового перемещения.
- Добавлена частичная перерисовка затронутых клеток; полная перерисовка остаётся запасным вариантом для крупных изменений.
- Убрано ненужное копирование кэшированных изображений значков при рисовании папки.
- Проверка заполненности корзины переведена с частого опроса на `inotify`, с запасной проверкой при отсутствии каталога.
- Геометрия мониторов для рабочего стола и обоев читается через RandR.
- Улучшена проверка актуальности PNG-кэша; обход каталогов защищён от циклических символических ссылок.
- Расширены проверки конфликтов файлов, групповой раскладки значков и PNG-кэша.

### Проверено

- Форматирование, Clippy, модульные тесты и X11/Xvfb-интеграция прошли в GitHub Actions для исходного программного коммита `a1fa5d91`.
- Тесты CI **не заменяют** испытания на реальной машине с разными дисплеями, файловыми системами и приложениями.

### Известные ограничения

- `cp/mv --no-clobber` вместе с проверками результата не обеспечивает полностью атомарное перемещение/копирование при всех гонках и сбоях.
- Значки рабочего стола сейчас размещаются на первом мониторе; полноценное распределение между мониторами не проверено и не завершено.
- Не подтверждены фактические показатели экономии RAM, CPU, энергии и плавности на пользовательском оборудовании.
- Совместимость и поведение собственного экрана блокировки требуют осторожности: X11 не гарантирует непрерывную защиту при перезапуске блокировщика.

## Как ведётся история

Каждое следующее заметное изменение добавляется отдельной записью с проверенными фактами. Запланированные функции и неподтверждённые улучшения производительности отмечаются в [NEXT.md](NEXT.md), а не выдаются за завершённые.
