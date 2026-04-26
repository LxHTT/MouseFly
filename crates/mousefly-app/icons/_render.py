#!/usr/bin/env python3
"""
Hand-rolled SVG → PNG rasterizer for MouseFly's logo. Stdlib only — we own
the geometry, no external deps.

Shapes are mirrored from icon.svg:
- rounded square background, vertical gradient
- 3 motion-trail dots
- 7-vertex cursor polygon with thin dark stroke

Run from this dir: `python3 _render.py` writes 32x32.png, 128x128.png,
128x128@2x.png (256), and icon.png (1024).
"""
import math
import os
import struct
import zlib

# --- shape definitions, in 256x256 reference space -----------------------

CURSOR = [
    (88, 60),
    (88, 196),
    (122, 162),
    (142, 204),
    (162, 196),
    (142, 154),
    (188, 148),
]
TRAIL = [(78, 184, 6, 0.18), (92, 170, 7, 0.32), (108, 154, 8, 0.55)]
RADIUS = 56  # rounded corner

BG_TOP = (0x1e, 0x40, 0xaf)      # blue-700
BG_BOT = (0x0b, 0x1d, 0x4f)      # slate-900-ish
CUR_TOP = (0xff, 0xff, 0xff)
CUR_BOT = (0xdb, 0xea, 0xfe)
STROKE = (0x0b, 0x1d, 0x4f)
STROKE_W = 3.0


def lerp(a, b, t):
    return a + (b - a) * t


def lerp_rgb(c1, c2, t):
    return tuple(int(round(lerp(c1[i], c2[i], t))) for i in range(3))


def in_rounded_rect(x, y, w, h, r):
    if 0 <= x < r and 0 <= y < r:
        return (x - r) * (x - r) + (y - r) * (y - r) <= r * r
    if w - r <= x < w and 0 <= y < r:
        return (x - (w - r - 1)) ** 2 + (y - r) ** 2 <= r * r
    if 0 <= x < r and h - r <= y < h:
        return (x - r) ** 2 + (y - (h - r - 1)) ** 2 <= r * r
    if w - r <= x < w and h - r <= y < h:
        return (x - (w - r - 1)) ** 2 + (y - (h - r - 1)) ** 2 <= r * r
    return 0 <= x < w and 0 <= y < h


def point_in_polygon(px, py, poly):
    inside = False
    n = len(poly)
    j = n - 1
    for i in range(n):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if ((yi > py) != (yj > py)) and (px < (xj - xi) * (py - yi) / (yj - yi + 1e-12) + xi):
            inside = not inside
        j = i
    return inside


def dist_to_segment(px, py, a, b):
    ax, ay = a
    bx, by = b
    dx, dy = bx - ax, by - ay
    L2 = dx * dx + dy * dy
    if L2 == 0:
        return math.hypot(px - ax, py - ay)
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / L2))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def dist_to_polygon_edge(px, py, poly):
    return min(
        dist_to_segment(px, py, poly[i], poly[(i + 1) % len(poly)])
        for i in range(len(poly))
    )


def alpha_at(px, py, scale, samples=4):
    """Compute pixel color and alpha for a SCALE-resolution image at (px, py).
    Subsamples SAMPLES x SAMPLES to anti-alias edges."""
    ref = 256.0
    contributions = []
    for sy in range(samples):
        for sx in range(samples):
            fx = (px + (sx + 0.5) / samples) * ref / scale
            fy = (py + (sy + 0.5) / samples) * ref / scale
            contributions.append(sample_color(fx, fy))
    # Average
    r = sum(c[0] * c[3] for c in contributions) / samples / samples
    g = sum(c[1] * c[3] for c in contributions) / samples / samples
    b = sum(c[2] * c[3] for c in contributions) / samples / samples
    a = sum(c[3] for c in contributions) / samples / samples
    if a <= 0:
        return (0, 0, 0, 0)
    # un-premultiply
    return (
        int(round(min(255, r / max(a, 1e-9)))),
        int(round(min(255, g / max(a, 1e-9)))),
        int(round(min(255, b / max(a, 1e-9)))),
        int(round(min(255, a * 255))),
    )


def sample_color(x, y):
    """Sample the layered logo at sub-pixel (x, y) in 256x256 space."""
    # Outside rounded rect → transparent
    if not in_rounded_rect(x, y, 256, 256, RADIUS):
        return (0, 0, 0, 0.0)
    # Background gradient
    bg = lerp_rgb(BG_TOP, BG_BOT, y / 256.0)
    out_r, out_g, out_b, out_a = bg[0], bg[1], bg[2], 1.0

    # Trail dots
    for cx, cy, r, op in TRAIL:
        d = math.hypot(x - cx, y - cy)
        if d < r:
            edge = max(0.0, min(1.0, r - d))
            a = op * edge
            out_r = out_r * (1 - a) + 255 * a
            out_g = out_g * (1 - a) + 255 * a
            out_b = out_b * (1 - a) + 255 * a

    # Cursor polygon: fill white→light-blue gradient inside, dark stroke on edge
    inside = point_in_polygon(x, y, CURSOR)
    edge_dist = dist_to_polygon_edge(x, y, CURSOR)
    fill_alpha = 0.0
    stroke_alpha = 0.0
    if inside:
        fill_alpha = 1.0
        if edge_dist < STROKE_W:
            stroke_alpha = 1.0 - edge_dist / STROKE_W
    elif edge_dist < STROKE_W:
        # outside, near edge: anti-aliased stroke
        stroke_alpha = 1.0 - edge_dist / STROKE_W
    if fill_alpha > 0:
        # gradient inside cursor (top→bottom)
        t = max(0.0, min(1.0, (x - 88) / 100.0 + (y - 60) / 144.0)) / 2.0
        cur = lerp_rgb(CUR_TOP, CUR_BOT, t)
        out_r = out_r * (1 - fill_alpha) + cur[0] * fill_alpha
        out_g = out_g * (1 - fill_alpha) + cur[1] * fill_alpha
        out_b = out_b * (1 - fill_alpha) + cur[2] * fill_alpha
    if stroke_alpha > 0:
        out_r = out_r * (1 - stroke_alpha) + STROKE[0] * stroke_alpha
        out_g = out_g * (1 - stroke_alpha) + STROKE[1] * stroke_alpha
        out_b = out_b * (1 - stroke_alpha) + STROKE[2] * stroke_alpha

    return (out_r, out_g, out_b, out_a)


# --- PNG output ----------------------------------------------------------

def write_png(path, size):
    print(f"writing {path} ({size}x{size})…")
    raw = bytearray()
    for y in range(size):
        raw.append(0)  # filter type
        for x in range(size):
            r, g, b, a = alpha_at(x, y, size, samples=3)
            raw.append(r)
            raw.append(g)
            raw.append(b)
            raw.append(a)

    def chunk(t, d):
        return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))

    sig = b'\x89PNG\r\n\x1a\n'
    ihdr = chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
    idat = chunk(b'IDAT', zlib.compress(bytes(raw), level=9))
    iend = chunk(b'IEND', b'')
    with open(path, 'wb') as f:
        f.write(sig + ihdr + idat + iend)


def write_tray_png(path, size):
    """Monochrome (black + alpha) cursor on transparent background — macOS
    template image. macOS auto-inverts based on the menu bar style."""
    print(f"writing {path} ({size}x{size}, template)…")
    raw = bytearray()
    samples = 4
    ref = 256.0
    for y in range(size):
        raw.append(0)
        for x in range(size):
            cov = 0.0
            for sy in range(samples):
                for sx in range(samples):
                    fx = (x + (sx + 0.5) / samples) * ref / size
                    fy = (y + (sy + 0.5) / samples) * ref / size
                    if not in_rounded_rect(fx, fy, 256, 256, RADIUS - 8):
                        # Tray template should be just the cursor — no bg.
                        pass
                    inside = point_in_polygon(fx, fy, CURSOR)
                    edge_dist = dist_to_polygon_edge(fx, fy, CURSOR)
                    if inside:
                        cov += 1.0
                    elif edge_dist < 1.0:
                        cov += 1.0 - edge_dist
            cov /= samples * samples
            alpha = int(round(min(255, cov * 255)))
            raw.append(0)  # R
            raw.append(0)  # G
            raw.append(0)  # B
            raw.append(alpha)

    def chunk(t, d):
        return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))

    sig = b'\x89PNG\r\n\x1a\n'
    ihdr = chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
    idat = chunk(b'IDAT', zlib.compress(bytes(raw), level=9))
    iend = chunk(b'IEND', b'')
    with open(path, 'wb') as f:
        f.write(sig + ihdr + idat + iend)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    app_sizes = {
        '32x32.png': 32,
        '128x128.png': 128,
        '128x128@2x.png': 256,
        'icon.png': 512,
    }
    for name, size in app_sizes.items():
        write_png(os.path.join(here, name), size)
    # macOS menu bar template: 32x32 logical (16pt @2x) is the conventional
    # tray icon size. Tauri's `icon_as_template(true)` flips it per-theme.
    write_tray_png(os.path.join(here, 'tray.png'), 32)


if __name__ == '__main__':
    main()
