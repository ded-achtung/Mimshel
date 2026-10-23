#!/usr/bin/env bash
# Установка shell на EndeavourOS / Arch Linux с Xfce (сеанс X11).
#
#   git clone https://github.com/ded-achtung/shell.git
#   cd shell && ./install.sh
#
# Что делает: ставит пакеты, собирает экран блокировки i3lock-color с нашей
# заплаткой, ставит значки, тему окон и курсор Colloid, собирает shell и
# включает её части (строка меню, док, уведомления, поиск, блокировка, обои).
# Перед изменениями сохраняет копию настроек: вернуть как было — shell restore.
# Повторный запуск безопасен: уже сделанное пропускается.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
SRC="$HOME/.local/share/shell/src"
BIN="$HOME/.local/bin/shell"

step() { printf '\n▶ %s\n' "$*"; }
ok() { printf '✔ %s\n' "$*"; }
warn() { printf '⚠ %s\n' "$*"; }
fail() { printf '✘ %s\n' "$*" >&2; exit 1; }

# --- Проверки ---------------------------------------------------------------
[ "$(id -u)" -ne 0 ] || fail "Запускайте от своего пользователя, не через sudo: пароль спросят, когда он понадобится."
command -v pacman >/dev/null || fail "Нужен Arch Linux или EndeavourOS (нет pacman)."
pgrep -u "$(id -u)" -x xfce4-session >/dev/null \
    || fail "Нужно войти в рабочий стол Xfce и запускать отсюда, из терминала Xfce."
[ "${XDG_SESSION_TYPE:-x11}" = x11 ] || fail "Нужен сеанс X11 (сейчас: $XDG_SESSION_TYPE). shell не работает в Wayland."
mkdir -p "$SRC" "$HOME/.local/bin"

# --- Пакеты -----------------------------------------------------------------
PKGS=(
    git base-devel rust          # сборка
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

# --- Экран блокировки: i3lock-color с заплаткой -----------------------------
# Заплатка: подсказка в поле сменяется точками ●, точки не вылезают за поле.
MARK="macos-look-field-v3"
if command -v i3lock >/dev/null && grep -q "$MARK" "$(command -v i3lock)"; then
    ok "i3lock-color с заплаткой уже стоит"
else
    step "Собираю i3lock-color из AUR с заплаткой (несколько минут)"
    d="$SRC/i3lock-color-aur"
    if [ -d "$d/.git" ]; then
        git -C "$d" pull --ff-only -q
    else
        git clone -q https://aur.archlinux.org/i3lock-color.git "$d"
    fi
    cp "$ROOT/packaging/i3lock-color-mac-field.patch" "$d/"
    cat "$d/PKGBUILD" - > "$d/PKGBUILD.shell" <<'EOF'

# --- shell: поле пароля как на Mac ---
pkgrel="${pkgrel}.1"
source+=("i3lock-color-mac-field.patch")
sha256sums+=('SKIP')
prepare() {
    cd "$srcdir/$pkgname-$pkgver"
    patch -Np1 -i "$srcdir/i3lock-color-mac-field.patch"
}
EOF
    rm -f "$d"/*.pkg.tar.*
    # -s — доставить нужное для сборки, -i — поставить (pacman спросит пароль).
    (cd "$d" && makepkg -f -s -i -p PKGBUILD.shell)
    grep -q "$MARK" "$(command -v i3lock)" || fail "i3lock-color не собрался с заплаткой"
    ok "i3lock-color с заплаткой поставлен"
fi

# --- Оформление Colloid: значки, курсор, тема окон --------------------------
fetch() { # fetch ИМЯ АДРЕС — скачать или обновить исходник в $SRC
    if [ -d "$SRC/$1/.git" ]; then
        git -C "$SRC/$1" pull --ff-only -q || true
    else
        git clone -q --depth 1 "$2" "$SRC/$1"
    fi
}
if [ -d "$HOME/.local/share/icons/Colloid-Dark" ]; then
    ok "Значки Colloid уже стоят"
else
    step "Значки Colloid"
    fetch Colloid-icon-theme https://github.com/vinceliuice/Colloid-icon-theme.git
    (cd "$SRC/Colloid-icon-theme" && ./install.sh)
fi
if [ -d "$HOME/.local/share/icons/Colloid-cursors" ]; then
    ok "Курсор Colloid уже стоит"
else
    step "Курсор Colloid"
    fetch Colloid-icon-theme https://github.com/vinceliuice/Colloid-icon-theme.git
    (cd "$SRC/Colloid-icon-theme/cursors" && ./install.sh)
fi
if [ -d "$HOME/.themes/Colloid-Dark" ]; then
    ok "Тема окон Colloid уже стоит"
else
    step "Тема окон Colloid (тёмная, обычные кнопки окна)"
    fetch Colloid-gtk-theme https://github.com/vinceliuice/Colloid-gtk-theme.git
    (cd "$SRC/Colloid-gtk-theme" && ./install.sh -c dark --tweaks normal)
fi

# --- Сама программа ---------------------------------------------------------
step "Собираю shell (первый раз — несколько минут)"
(cd "$ROOT" && cargo build --release --locked)
install -m 755 "$ROOT/target/release/shell" "$BIN"
ok "Программа: $BIN"

# --- Настройки и включение --------------------------------------------------
step "Копия нынешних настроек (вернуть как было: $BIN restore)"
"$BIN" backup pered-shell

step "Включаю части shell"
for part in dock menubar notify spotlight launchpad; do
    "$BIN" set --no-apply "parts.$part" shell
done
"$BIN" set --no-apply look.icon_theme Colloid-shell
"$BIN" set --no-apply look.gtk_theme Colloid-Dark
"$BIN" set --no-apply look.cursor_theme Colloid-cursors
"$BIN" set --no-apply look.wallpaper space
"$BIN" apply

step "Проверка"
"$BIN" doctor || true

cat <<'EOF'

✔ Готово. Выйдите из системы и войдите снова: меню программ в строке меню,
  блокировка по бездействию и служба shell включаются при входе.

  Проверить, что всё на месте:  ~/.local/bin/shell doctor
  Вернуть как было:              ~/.local/bin/shell restore
EOF
