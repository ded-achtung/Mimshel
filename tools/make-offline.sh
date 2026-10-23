#!/usr/bin/env bash
# Архив для установки без доступа к GitHub и crates.io: проект, исходники
# значков и темы Colloid, все библиотеки Rust. Из интернета установщику
# остаются нужны только пакеты системы (pacman).
#
#   tools/make-offline.sh            → dist/shell-offline.tar.gz
#
# На новой машине: tar xzf shell-offline.tar.gz && cd shell && ./install.sh

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$HOME/.local/share/shell/src"
OUT="$ROOT/dist"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cd "$ROOT"
[ -z "$(git status --porcelain)" ] || echo "⚠ В проекте есть незакоммиченные изменения — в архив попадёт последний коммит"

echo "▶ Проект (последний коммит)"
mkdir -p "$WORK/shell"
git archive HEAD | tar x -C "$WORK/shell"

echo "▶ Значки и тема Colloid"
for name in Colloid-icon-theme Colloid-gtk-theme; do
    if [ -d "$SRC/$name" ]; then
        cp -r "$SRC/$name" "$WORK/shell/offline-$name.tmp"
    else
        git clone -q --depth 1 "https://github.com/vinceliuice/$name.git" "$WORK/shell/offline-$name.tmp"
    fi
    mkdir -p "$WORK/shell/offline"
    rm -rf "$WORK/shell/offline-$name.tmp/.git"
    mv "$WORK/shell/offline-$name.tmp" "$WORK/shell/offline/$name"
done

echo "▶ Библиотеки Rust"
cargo vendor --locked --offline "$WORK/shell/offline/vendor" >/dev/null

mkdir -p "$OUT"
tar czf "$OUT/shell-offline.tar.gz" -C "$WORK" shell
echo "✔ $OUT/shell-offline.tar.gz ($(du -h "$OUT/shell-offline.tar.gz" | cut -f1))"
