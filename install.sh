#!/usr/bin/env bash
# Установка Mimshel на EndeavourOS / Arch Linux с Xfce (сеанс X11).
#
#   git clone https://github.com/ded-achtung/Mimshel.git
#   cd Mimshel && ./install.sh
#
# Если GitHub недоступен: архив со всем нужным собирает tools/make-offline.sh
# (проект, значки и тема Colloid, библиотеки Rust) — распаковать и ./install.sh.
#
# Что делает: ставит пакеты, значки, тему окон и курсор Colloid, собирает Mimshel
# и включает его части (строка меню, док, рабочий стол, Проводник, уведомления,
# поиск, экран блокировки, обои).
# Перед изменениями сохраняет копию настроек: вернуть как было — mimshel restore.
# Повторный запуск безопасен: уже сделанное пропускается.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DATA="$HOME/.local/share/mimshel"
SRC="$DATA/src"
BIN="$HOME/.local/bin/mimshel"

step() { printf '\n▶ %s\n' "$*"; }
ok() { printf '✔ %s\n' "$*"; }
warn() { printf '⚠ %s\n' "$*"; }
fail() { printf '✘ %s\n' "$*" >&2; exit 1; }

# --- Проверки ---------------------------------------------------------------
[ "$(id -u)" -ne 0 ] || fail "Запускайте от своего пользователя, не через sudo: пароль спросят, когда он понадобится."
command -v pacman >/dev/null || fail "Нужен Arch Linux или EndeavourOS (нет pacman)."
pgrep -u "$(id -u)" -x xfce4-session >/dev/null \
    || fail "Нужно войти в рабочий стол Xfce и запускать отсюда, из терминала Xfce."
[ "${XDG_SESSION_TYPE:-x11}" = x11 ] || fail "Нужен сеанс X11 (сейчас: $XDG_SESSION_TYPE). Mimshel не работает в Wayland."
# Папка данных до переименования проекта (~/.local/share/shell) — под новым именем.
# Только нашу: в ней лежат копии настроек или главное меню.
if [ ! -e "$DATA" ] && { [ -d "$HOME/.local/share/shell/backups" ] || [ -e "$HOME/.local/share/shell/apple.menu" ]; }; then
    mv "$HOME/.local/share/shell" "$DATA"
fi
mkdir -p "$SRC" "$HOME/.local/bin"

# --- Пакеты -----------------------------------------------------------------
# With rustup's toolchain already here the `rust` package conflicts with it:
# pacman would ask to remove rustup, and «no» ends the script.
RUST_PKG=(rust)
if command -v rustup >/dev/null && command -v cargo >/dev/null; then
    RUST_PKG=()
fi
PKGS=(
    git base-devel "${RUST_PKG[@]}" clang  # сборка (clang — для привязки к PAM)
    pam                          # проверка пароля на экране блокировки
    sassc                        # сборка темы окон Colloid
    inter-font                   # шрифт
    zenity                       # окна «Выключить?», «Об этом компьютере»
    xss-lock xorg-xset           # блокировка по бездействию и перед сном
    xorg-xrandr xorg-xkbcomp xorg-xinput  # экран и раскладки
    appmenu-gtk-module           # меню программ — в строку меню
    network-manager-applet       # значок сети
    xfce4-taskmanager            # «Принудительно завершить»
    pavucontrol                  # «Настройки звука»
)
step "Пакеты (pacman спросит пароль sudo и подтверждение)"
# -Syu, а не -Sy: частичное обновление ломает Arch.
sudo pacman -Syu --needed "${PKGS[@]}"
ok "Пакеты на месте"

# --- Оформление Colloid: значки, курсор, тема окон --------------------------
# Pinned commits of the third-party sources (COLLOID_*_URL/REV).
. "$ROOT/packaging/third-party.sh"

# Commit a source copy in $SRC is at: git HEAD, or the marker that
# tools/make-offline.sh leaves in copies without .git.
rev_of() {
    if [ -d "$1/.git" ]; then
        git -C "$1" rev-parse HEAD 2>/dev/null
    else
        cat "$1/.mimshel-rev" 2>/dev/null
    fi
}
fetch() { # fetch NAME URL REV — put exactly commit REV of NAME into $SRC/NAME
    local dir="$SRC/$1"
    [ "$(rev_of "$dir")" = "$3" ] && return
    rm -rf "$dir"
    if [ -d "$ROOT/offline/$1" ]; then
        # Offline archive (tools/make-offline.sh): the source is already here.
        [ "$(cat "$ROOT/offline/$1/.mimshel-rev" 2>/dev/null)" = "$3" ] \
            || fail "В архиве другая версия $1, чем ждёт установщик. Соберите архив заново: tools/make-offline.sh"
        cp -r "$ROOT/offline/$1" "$dir"
    else
        git init -q "$dir"
        git -C "$dir" fetch -q --depth 1 "$2" "$3"
        git -C "$dir" checkout -q --detach FETCH_HEAD
    fi
    [ "$(rev_of "$dir")" = "$3" ] || fail "$1: скачалась не та версия (ждали $3)."
}
# install_theme LABEL NAME URL REV SUBDIR DIR [ARGS…] — run SUBDIR/install.sh ARGS
# of source NAME at commit REV, unless the theme in DIR was installed from that
# very commit (the marker DIR/.mimshel-rev). "Already there" is not enough: a
# machine that installed once would stay on the old commit for ever after the
# pin in packaging/third-party.sh moves. A theme installed before the markers
# existed has none and is installed again, once.
# By a file inside, not by the folder: an interrupted install leaves the
# folder behind.
install_theme() {
    local label="$1" name="$2" url="$3" rev="$4" sub="$5" dir="$6"
    shift 6
    if [ -f "$dir/index.theme" ] && [ "$(cat "$dir/.mimshel-rev" 2>/dev/null)" = "$rev" ]; then
        ok "$label: уже стоит"
        return
    fi
    step "$label"
    # In a subshell: without the network the theme that is already there stays.
    if (fetch "$name" "$url" "$rev" && cd "$SRC/$name/$sub" && ./install.sh "$@"); then
        printf '%s\n' "$rev" >"$dir/.mimshel-rev"
    elif [ -f "$dir/index.theme" ]; then
        warn "$label: не обновлено (нет сети?) — остаётся то, что стояло"
    else
        fail "$label: не установлено"
    fi
}
# Colloid-Dark links into Colloid-Light (lesson 28): without it the dark
# icons are broken, so its absence means "install again".
[ -f "$HOME/.local/share/icons/Colloid-Light/index.theme" ] \
    || rm -f "$HOME/.local/share/icons/Colloid-Dark/.mimshel-rev"
install_theme "Значки Colloid" Colloid-icon-theme "$COLLOID_ICON_URL" "$COLLOID_ICON_REV" \
    . "$HOME/.local/share/icons/Colloid-Dark"
install_theme "Курсор Colloid" Colloid-icon-theme "$COLLOID_ICON_URL" "$COLLOID_ICON_REV" \
    cursors "$HOME/.local/share/icons/Colloid-cursors"
install_theme "Тема окон Colloid (тёмная, обычные кнопки окна)" Colloid-gtk-theme \
    "$COLLOID_GTK_URL" "$COLLOID_GTK_REV" . "$HOME/.themes/Colloid-Dark" -c dark --tweaks normal

# --- Сама программа ---------------------------------------------------------
step "Собираю Mimshel (первый раз — несколько минут)"
# Vendored crates only if they were made for this Cargo.lock: a stale
# offline/ folder (an older archive unpacked here) must not break the build.
vendor_ok() {
    [ -d "$ROOT/offline/vendor" ] && [ -f "$ROOT/offline/vendor.lock-sha256" ] \
        && [ "$(cat "$ROOT/offline/vendor.lock-sha256")" = "$(sha256sum <"$ROOT/Cargo.lock" | cut -d' ' -f1)" ]
}
if [ -d "$ROOT/offline/vendor" ] && ! vendor_ok; then
    warn "offline/vendor собран для другой версии — собираю с crates.io"
fi
# Only the program this script installs: the compositor (another member of
# the workspace) is not installed and would only add build time.
if vendor_ok && [ -f "$ROOT/offline/cargo-config.toml" ]; then
    # Библиотеки Rust из архива — без обращения к crates.io и GitHub: the
    # settings made by tools/make-offline.sh redirect every source, the git
    # one (Smithay) too.
    VENDOR_CONFIG="$(mktemp --suffix=.toml)"
    sed "s|@VENDOR@|$ROOT/offline/vendor|" "$ROOT/offline/cargo-config.toml" >"$VENDOR_CONFIG"
    (cd "$ROOT" && cargo build --release --locked --offline -p mimshel --config "$VENDOR_CONFIG")
    rm -f "$VENDOR_CONFIG"
elif vendor_ok; then
    # An archive made before cargo-config.toml existed: crates.io only.
    (cd "$ROOT" && cargo build --release --locked --offline -p mimshel \
        --config 'source.crates-io.replace-with="vendored"' \
        --config 'source.vendored.directory="offline/vendor"')
else
    (cd "$ROOT" && cargo build --release --locked -p mimshel)
fi
install -m 755 "$ROOT/target/release/mimshel" "$BIN"
# Прежняя команда shell — ссылкой на новую (старые сочетания клавиш, записки).
# Чужой файл с таким именем не трогаем.
if [ -L "$HOME/.local/bin/shell" ] || [ ! -e "$HOME/.local/bin/shell" ]; then
    ln -sfn mimshel "$HOME/.local/bin/shell"
fi
ok "Программа: $BIN"

# --- Настройки и включение --------------------------------------------------
# Before any mimshel command: the first one writes the settings file.
if [ -f "${MIMSHEL_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/mimshel/config.toml}" ]; then
    FIRST_INSTALL=no
else
    FIRST_INSTALL=yes
fi
step "Копия нынешних настроек (вернуть как было: $BIN restore)"
"$BIN" backup pered-mimshel

# Parts and look are chosen for the user only on a first install. Run again
# (an update, a missing package) the script keeps what they have chosen
# since: it used to put all eleven settings back.
if [ "$FIRST_INSTALL" = yes ]; then
    step "Включаю части Mimshel"
    for part in dock menubar notify desktop folders search_panel apps_panel; do
        "$BIN" set --no-apply "parts.$part" mimshel
    done
    "$BIN" set --no-apply look.icon_theme Colloid-mimshel
    "$BIN" set --no-apply look.gtk_theme Colloid-Dark
    "$BIN" set --no-apply look.cursor_theme Colloid-cursors
    "$BIN" set --no-apply look.wallpaper space
else
    step "Настройки уже есть — части и оформление остаются как выбраны"
fi
# apply now says so when a step failed; the check below shows what is left.
"$BIN" apply || warn "Не всё применилось — смотрите строки с ✘ выше и проверку ниже"

step "Проверка"
"$BIN" doctor || true

cat <<'EOF'

✔ Готово. Выйдите из системы и войдите снова: меню программ в строке меню,
  блокировка по бездействию и служба Mimshel включаются при входе.

  Проверить, что всё на месте:  ~/.local/bin/mimshel doctor
  Вернуть как было:              ~/.local/bin/mimshel restore
EOF
