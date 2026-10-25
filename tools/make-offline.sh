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
SRC="$HOME/.local/share/mimshel/src"
[ -d "$SRC" ] || SRC="$HOME/.local/share/shell/src"
OUT="$ROOT/dist"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cd "$ROOT"
[ -z "$(git status --porcelain)" ] || echo "⚠ В проекте есть незакоммиченные изменения — в архив попадёт последний коммит"

echo "▶ Проект (последний коммит)"
mkdir -p "$WORK/Mimshel"
git archive HEAD | tar x -C "$WORK/Mimshel"

echo "▶ Значки и тема Colloid"
for name in Colloid-icon-theme Colloid-gtk-theme; do
    if [ -d "$SRC/$name" ]; then
        cp -r "$SRC/$name" "$WORK/Mimshel/offline-$name.tmp"
    else
        git clone -q --depth 1 "https://github.com/vinceliuice/$name.git" "$WORK/Mimshel/offline-$name.tmp"
    fi
    mkdir -p "$WORK/Mimshel/offline"
    rm -rf "$WORK/Mimshel/offline-$name.tmp/.git"
    mv "$WORK/Mimshel/offline-$name.tmp" "$WORK/Mimshel/offline/$name"
done

echo "▶ Библиотеки Rust"
cargo vendor --quiet --locked --offline "$WORK/Mimshel/offline/vendor" >/dev/null

mkdir -p "$OUT"
tar czf "$OUT/mimshel-offline.tar.gz" -C "$WORK" Mimshel
echo "✔ $OUT/mimshel-offline.tar.gz ($(du -h "$OUT/mimshel-offline.tar.gz" | cut -f1))"
