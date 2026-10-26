#!/usr/bin/env bash
# Архив для установки без доступа к GitHub и crates.io: проект, исходники
# значков и темы Colloid, все библиотеки Rust. Из интернета установщику
# остаются нужны только пакеты системы (pacman).
#
#   tools/make-offline.sh            → dist/mimshel-offline.tar.gz
#
# На новой машине: tar xzf mimshel-offline.tar.gz && cd Mimshel && ./install.sh

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/dist"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cd "$ROOT"
[ -z "$(git status --porcelain)" ] || echo "⚠ В проекте есть незакоммиченные изменения — в архив попадёт последний коммит"

echo "▶ Проект (последний коммит)"
mkdir -p "$WORK/Mimshel"
git archive HEAD | tar x -C "$WORK/Mimshel"

echo "▶ Значки и тема Colloid"
. "$ROOT/packaging/third-party.sh"
mkdir -p "$WORK/Mimshel/offline"
pack() { # pack NAME URL REV — exactly the pinned commit, never a local copy
    local tmp="$WORK/Mimshel/offline-$1.tmp"
    git init -q "$tmp"
    git -C "$tmp" fetch -q --depth 1 "$2" "$3"
    git -C "$tmp" checkout -q --detach FETCH_HEAD
    [ "$(git -C "$tmp" rev-parse HEAD)" = "$3" ] || { echo "✘ $1: не та версия" >&2; exit 1; }
    rm -rf "$tmp/.git"
    # install.sh checks this marker before using the copy.
    echo "$3" >"$tmp/.mimshel-rev"
    mv "$tmp" "$WORK/Mimshel/offline/$1"
}
pack Colloid-icon-theme "$COLLOID_ICON_URL" "$COLLOID_ICON_REV"
pack Colloid-gtk-theme "$COLLOID_GTK_URL" "$COLLOID_GTK_REV"

echo "▶ Библиотеки Rust"
cargo vendor --quiet --locked --offline "$WORK/Mimshel/offline/vendor" >/dev/null
# install.sh uses the vendored crates only for this exact Cargo.lock.
sha256sum <Cargo.lock | cut -d' ' -f1 >"$WORK/Mimshel/offline/vendor.lock-sha256"

mkdir -p "$OUT"
tar czf "$OUT/mimshel-offline.tar.gz" -C "$WORK" Mimshel
# Checksum to publish next to the archive (check: sha256sum -c).
(cd "$OUT" && sha256sum mimshel-offline.tar.gz >mimshel-offline.tar.gz.sha256)
echo "✔ $OUT/mimshel-offline.tar.gz ($(du -h "$OUT/mimshel-offline.tar.gz" | cut -f1))"
echo "  SHA-256: $(cut -d' ' -f1 "$OUT/mimshel-offline.tar.gz.sha256")"
