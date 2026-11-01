#!/bin/sh
# Try the Wayland side of Mimshel without leaving the X11 session: our
# compositor in a window (docs/compositor.md), inside it a second Mimshel
# service with only the menu bar, our terminal as a Wayland client and the
# same terminal as an X11 program through XWayland.
#
# Nothing of the real session is shared: the second service gets its own
# runtime, state and cache folders, its own copy of the config (dock,
# desktop, notifications and folder windows switched off) and its own D-Bus
# (dbus-run-session), so it does not compete with the running service for
# the tray, the global menu or notifications.
#
# Usage: tools/nested-session.sh [TARGET_DIR]   (default target/release)
# Close the compositor's window to end everything.
set -eu
R=${1:-$(dirname "$0")/../target/release}
R=$(cd "$R" && pwd)
N=${TMPDIR:-/tmp}/mimshel-nested
rm -rf "$N"
mkdir -p "$N/rt" "$N/state" "$N/cache"
chmod 700 "$N/rt"
CONFIG=${XDG_CONFIG_HOME:-$HOME/.config}/mimshel/config.toml
sed -E 's/^(desktop|notify) = .*/\1 = "xfce"/; s/^dock = .*/dock = "plank"/; s/^folders = .*/folders = "thunar"/' \
    "$CONFIG" > "$N/config.toml"
# Hybrid laptops: only Mesa's EGL, so NVIDIA's driver is not loaded (and the
# dGPU not woken) for a compositor drawing on the integrated GPU.
MESA=/usr/share/glvnd/egl_vendor.d/50_mesa.json
[ -e "$MESA" ] && export __EGL_VENDOR_LIBRARY_FILENAMES="$MESA"
setsid "$R/mimshel-compositor" --nested > "$N/compositor.log" 2>&1 &
i=0
until grep -q '^WAYLAND_DISPLAY=' "$N/compositor.log" 2>/dev/null; do
    i=$((i + 1))
    [ "$i" -gt 50 ] && { echo "the compositor did not start:"; cat "$N/compositor.log"; exit 1; }
    sleep 0.1
done
WD=$(sed -n 's/^WAYLAND_DISPLAY=\([^ ]*\).*/\1/p' "$N/compositor.log")
XD=$(sed -n 's/.* DISPLAY=\(:[0-9]*\).*/\1/p' "$N/compositor.log")
RUNTIME=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
echo "inside: WAYLAND_DISPLAY=$WD DISPLAY=$XD; logs in $N"
iso() {
    env XDG_RUNTIME_DIR="$N/rt" XDG_STATE_HOME="$N/state" XDG_CACHE_HOME="$N/cache" \
        MIMSHEL_CONFIG="$N/config.toml" "$@"
}
iso env -u DISPLAY WAYLAND_DISPLAY="$RUNTIME/$WD" \
    setsid dbus-run-session -- "$R/mimshel" daemon --child > "$N/service.log" 2>&1 &
sleep 2
iso env -u DISPLAY WAYLAND_DISPLAY="$RUNTIME/$WD" setsid "$R/mimshel" terminal \
    > "$N/term-wayland.log" 2>&1 &
if [ -n "$XD" ]; then
    iso env -u WAYLAND_DISPLAY DISPLAY="$XD" MIMSHEL_BACKEND=x11 setsid "$R/mimshel" terminal \
        > "$N/term-x11.log" 2>&1 &
fi
