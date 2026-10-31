#!/bin/sh
# Idle CPU and memory of the running Mimshel service, to compare builds.
# Usage: tools/measure-service.sh [SAMPLES] [SECONDS]  (default 6 × 10 s)
# Prints PSS (kB), threads and open files per sample — all three must stay
# flat (growth means a leak) — then CPU % of the service and its guard over
# the whole window. CLAUDE.md, "Measurements": PSS from smaps_rollup, CPU
# from /proc/PID/stat ticks.
set -eu
bin="$HOME/.local/bin/mimshel"
P=$(pgrep -f "^$bin daemon --child")
G=$(pgrep -f "^$bin daemon\$")
HZ=$(getconf CLK_TCK)
ticks() { awk '{print $14 + $15}' "/proc/$1/stat"; }
pss() { awk '/^Pss:/ {print $2}' "/proc/$1/smaps_rollup"; }
echo "service $P, guard $G, $("$bin" --version), $(sha256sum "$bin" | cut -c1-12)"
n=${1:-6}
step=${2:-10}
t0=$(ticks "$P")
g0=$(ticks "$G")
s0=$(date +%s.%N)
i=1
while [ "$i" -le "$n" ]; do
    sleep "$step"
    echo "  t=$((i * step))s PSS=$(pss "$P") kB threads=$(ls "/proc/$P/task" | wc -l) fds=$(ls "/proc/$P/fd" | wc -l)"
    i=$((i + 1))
done
t1=$(ticks "$P")
g1=$(ticks "$G")
s1=$(date +%s.%N)
awk -v a="$t0" -v b="$t1" -v c="$g0" -v d="$g1" -v s="$s0" -v e="$s1" -v hz="$HZ" -v gp="$(pss "$G")" \
    'BEGIN { printf "service CPU %.2f %%, guard CPU %.2f %%, guard PSS %s kB\n", (b - a) * 100 / hz / (e - s), (d - c) * 100 / hz / (e - s), gp }'
