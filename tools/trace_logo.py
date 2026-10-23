#!/usr/bin/env python3
"""Переводит знак (SVG с масками и линиями) в одну залитую фигуру — для
одноцветных значков: GTK перекрашивает их, заливая все фигуры одним цветом,
и маски с линиями при этом ломаются. Рисует знак крупно (rsvg-convert),
проводит контур по краю (marching squares) и упрощает его.

    tools/trace_logo.py вход.svg выход.svg [цвет]
"""
import subprocess
import sys

N = 512          # размер растра для обводки
VIEW = 64        # размер viewBox знака


def alpha_grid(svg):
    png = subprocess.run(["rsvg-convert", "-w", str(N), "-h", str(N), svg],
                         check=True, capture_output=True).stdout
    pgm = subprocess.run(["magick", "png:-", "-alpha", "extract", "-depth", "8", "pgm:-"],
                         input=png, check=True, capture_output=True).stdout
    # Заголовок P5: магия, ширина высота, максимум, затем байты.
    parts = pgm.split(maxsplit=4)
    w, h = int(parts[1]), int(parts[2])
    data = parts[4]
    return [[data[y * w + x] / 255.0 for x in range(w)] for y in range(h)]


def contours(a):
    """Отрезки границы на уровне 0,5 (с подсчётом точки внутри клетки)."""
    h, w = len(a), len(a[0])
    v = lambda x, y: a[y][x] if 0 <= x < w and 0 <= y < h else 0.0
    segs = []
    for y in range(-1, h):
        for x in range(-1, w):
            c = [v(x, y), v(x + 1, y), v(x + 1, y + 1), v(x, y + 1)]
            idx = sum(1 << i for i, cv in enumerate(c) if cv >= 0.5)
            if idx in (0, 15):
                continue
            def lerp(i, j, p, q):
                t = (0.5 - c[i]) / (c[j] - c[i]) if c[j] != c[i] else 0.5
                return (p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t)
            P = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)]
            e = {0: lerp(0, 1, P[0], P[1]), 1: lerp(1, 2, P[1], P[2]),
                 2: lerp(3, 2, P[3], P[2]), 3: lerp(0, 3, P[0], P[3])}
            table = {1: [(3, 0)], 2: [(0, 1)], 3: [(3, 1)], 4: [(1, 2)], 5: [(3, 0), (1, 2)],
                     6: [(0, 2)], 7: [(3, 2)], 8: [(2, 3)], 9: [(2, 0)], 10: [(0, 1), (2, 3)],
                     11: [(2, 1)], 12: [(1, 3)], 13: [(1, 0)], 14: [(0, 3)]}
            for s, t in table[idx]:
                segs.append((e[s], e[t]))
    return segs


def chain(segs):
    key = lambda p: (round(p[0], 4), round(p[1], 4))
    nxt = {}
    for s, t in segs:
        nxt[key(s)] = (s, t)
    loops, seen = [], set()
    for k in list(nxt):
        if k in seen:
            continue
        loop, cur = [], k
        while cur in nxt and cur not in seen:
            seen.add(cur)
            s, t = nxt[cur]
            loop.append(s)
            cur = key(t)
        if len(loop) > 2:
            loops.append(loop)
    return loops


def rdp(pts, eps):
    if len(pts) < 3:
        return pts
    (x1, y1), (x2, y2) = pts[0], pts[-1]
    dx, dy = x2 - x1, y2 - y1
    norm = (dx * dx + dy * dy) ** 0.5 or 1e-9
    dmax, idx = 0.0, 0
    for i in range(1, len(pts) - 1):
        d = abs(dy * pts[i][0] - dx * pts[i][1] + x2 * y1 - y2 * x1) / norm
        if d > dmax:
            dmax, idx = d, i
    if dmax > eps:
        return rdp(pts[: idx + 1], eps)[:-1] + rdp(pts[idx:], eps)
    return [pts[0], pts[-1]]


def main():
    src, out = sys.argv[1], sys.argv[2]
    color = sys.argv[3] if len(sys.argv) > 3 else "#dedede"
    k = VIEW / N
    paths = []
    for loop in chain(contours(alpha_grid(src))):
        # Замкнутый контур упрощаем двумя половинами, чтобы концы не слиплись.
        half = len(loop) // 2
        pts = rdp(loop[: half + 1], 0.35) + rdp(loop[half:] + [loop[0]], 0.35)[1:-1]
        d = "M" + " ".join(f"{x * k:.2f} {y * k:.2f}" for x, y in pts) + "Z"
        paths.append(d)
    with open(out, "w") as f:
        f.write(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {VIEW} {VIEW}" '
                f'width="{VIEW}" height="{VIEW}">\n'
                f'  <!-- Знак shell одной залитой фигурой (tools/trace_logo.py) -->\n'
                f'  <path fill="{color}" fill-rule="evenodd" d="{" ".join(paths)}"/>\n</svg>\n')
    print(f"контуров: {len(paths)}, точек: {sum(p.count(' ') for p in paths) // 2}")


main()
