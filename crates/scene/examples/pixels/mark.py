# Draws the thc mark, [•], as a PNG (supersampled, anti-aliased) with only the standard library.
import math, struct, sys, zlib
W, H, SS = 720, 240, 3
# mark.py [out.png] [accent as r,g,b]
MUTED = (160, 150, 138)
ACCENT = tuple(int(v) for v in sys.argv[2].split(",")) if len(sys.argv) > 2 else (232, 131, 79)
def png(path, px):
    raw = b"".join(b"\x00" + bytes(v for p in row for v in p) for row in px)
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0)) +
                           chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))
def shade(x, y):
    """Colour and coverage at a point (in output pixels)."""
    # Brackets: rounded strokes.
    t = 22
    for sx, d in ((150, 1), (570, -1)):
        bar = abs(x - sx) <= t / 2 and 30 - t / 2 <= y <= H - 30 + t / 2
        arms = (abs(y - 30) <= t / 2 or abs(y - (H - 30)) <= t / 2) and 0 <= (x - sx) * d <= 70
        if bar or arms: return MUTED, 1.0
    # The dot: a lit sphere with a glow.
    cx, cy, r = W / 2, H / 2, 52
    dist = math.hypot(x - cx, y - cy)
    if dist <= r:
        hl = math.hypot(x - (cx - 18), y - (cy - 18)) / (r * 1.3)
        k = min(1.0, hl)
        return tuple(round(255 - (255 - c) * k ** 0.8) if i < 3 else c for i, c in enumerate(ACCENT)), 1.0
    if dist <= r * 2.4:
        return ACCENT, ((1 - (dist - r) / (r * 1.4)) ** 2) * 0.45
    return (0, 0, 0), 0.0
rows = []
for y in range(H):
    row = []
    for x in range(W):
        acc = [0.0, 0.0, 0.0, 0.0]
        for sy in range(SS):
            for sx in range(SS):
                c, a = shade(x + (sx + 0.5) / SS, y + (sy + 0.5) / SS)
                for i in range(3): acc[i] += c[i] * a
                acc[3] += a
        a = acc[3] / SS ** 2
        row.append(tuple(round(acc[i] / acc[3]) if acc[3] else 0 for i in range(3)) + (round(a * 255),))
    rows.append(row)
png(sys.argv[1] if len(sys.argv) > 1 else "mark.png", rows)
