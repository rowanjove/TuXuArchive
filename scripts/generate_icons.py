#!/usr/bin/env python3
"""Generate clean white-background blue brand icons for 图序归档 (TuXuArchive)."""

import math
import os
import struct
import zlib


def clamp(v, lo=0.0, hi=1.0):
    return lo if v < lo else (hi if v > hi else v)


def smoothstep(edge0, edge1, x):
    t = clamp((x - edge0) / (edge1 - edge0))
    return t * t * (3.0 - 2.0 * t)


def sd_rounded_box(px, py, cx, cy, hw, hh, r):
    dx = abs(px - cx) - (hw - r)
    dy = abs(py - cy) - (hh - r)
    ax = max(dx, 0.0)
    ay = max(dy, 0.0)
    return math.hypot(ax, ay) + min(max(dx, dy), 0.0) - r


def sd_circle(px, py, cx, cy, r):
    return math.hypot(px - cx, py - cy) - r


def sd_segment(px, py, x1, y1, x2, y2):
    vx, vy = x2 - x1, y2 - y1
    wx, wy = px - x1, py - y1
    c1 = vx * wx + vy * wy
    if c1 <= 0.0:
        return math.hypot(px - x1, py - y1)
    c2 = vx * vx + vy * vy
    if c2 <= c1:
        return math.hypot(px - x2, py - y2)
    b = c1 / c2
    bx, by = x1 + b * vx, y1 + b * vy
    return math.hypot(px - bx, py - by)


def blend(dst, src_rgb, alpha):
    if alpha <= 0.0:
        return dst
    a = clamp(alpha)
    inv = 1.0 - a
    return (
        dst[0] * inv + src_rgb[0] * a,
        dst[1] * inv + src_rgb[1] * a,
        dst[2] * inv + src_rgb[2] * a,
        clamp(dst[3] + a * (1.0 - dst[3])),
    )


def sample_icon(u, v, px_size):
    """Sample normalized [0,1]^2 icon coordinate."""
    aa = 1.25 / px_size

    # Outer white rounded tile with crisp subtle cool border
    d_tile = sd_rounded_box(u, v, 0.5, 0.5, 0.46, 0.46, 0.19)
    tile_alpha = 1.0 - smoothstep(-aa, aa, d_tile)
    if tile_alpha <= 0.001:
        return (0.0, 0.0, 0.0, 0.0)

    # Pure white background with a subtle cool top-to-bottom sheen
    bg_r = 255.0 - 4.0 * v
    bg_g = 255.0 - 2.0 * v
    bg_b = 255.0
    col = (bg_r, bg_g, bg_b, tile_alpha)

    # Subtle border inside the white rounded tile
    border_band = smoothstep(-aa, aa, abs(d_tile + 0.008) - 0.007)
    border_alpha = (1.0 - border_band) * tile_alpha * 0.85
    col = blend(col, (214.0, 226.0, 244.0), border_alpha)

    # Graphic motif: Two ordered blue photo/archive layers + foreground card + sequence badge
    # 1) Back photo card (soft sky blue, shifted upper-right)
    d_back = sd_rounded_box(u, v, 0.56, 0.34, 0.22, 0.16, 0.04)
    a_back = (1.0 - smoothstep(-aa, aa, d_back)) * tile_alpha
    col = blend(col, (147.0, 197.0, 253.0), a_back)

    # 2) Middle photo card (medium cobalt blue)
    d_mid = sd_rounded_box(u, v, 0.50, 0.41, 0.24, 0.17, 0.045)
    a_mid = (1.0 - smoothstep(-aa, aa, d_mid)) * tile_alpha
    col = blend(col, (59.0, 130.0, 246.0), a_mid)

    # 3) Foreground archive folder / ordered frame (deep crisp blue)
    d_folder_body = sd_rounded_box(u, v, 0.48, 0.58, 0.28, 0.20, 0.055)
    d_folder_tab = sd_rounded_box(u, v, 0.32, 0.40, 0.12, 0.06, 0.03)
    d_folder = min(d_folder_body, d_folder_tab)
    a_folder = (1.0 - smoothstep(-aa, aa, d_folder)) * tile_alpha
    # Vertical blue gradient for depth
    fg_r = 29.0 - 11.0 * clamp((v - 0.35) / 0.45)
    fg_g = 99.0 - 25.0 * clamp((v - 0.35) / 0.45)
    fg_b = 237.0 - 38.0 * clamp((v - 0.35) / 0.45)
    col = blend(col, (fg_r, fg_g, fg_b), a_folder)

    # 4) Crisp white inner photo frame & sequence lines inside the blue folder
    # Left mini photo emblem (white outline + mountain + sun)
    d_photo_outer = sd_rounded_box(u, v, 0.36, 0.59, 0.105, 0.105, 0.025)
    d_photo_inner = sd_rounded_box(u, v, 0.36, 0.59, 0.080, 0.080, 0.015)
    d_photo_ring = max(d_photo_outer, -d_photo_inner)
    a_photo_ring = (1.0 - smoothstep(-aa, aa, d_photo_ring)) * a_folder
    col = blend(col, (255.0, 255.0, 255.0), a_photo_ring)

    # Sun dot inside mini photo
    d_sun = sd_circle(u, v, 0.325, 0.555, 0.018)
    a_sun = (1.0 - smoothstep(-aa, aa, d_sun)) * a_folder
    col = blend(col, (255.0, 255.0, 255.0), a_sun)

    # Stylized mountain ridge inside mini photo
    d_m1 = sd_segment(u, v, 0.295, 0.648, 0.355, 0.582) - 0.013
    d_m2 = sd_segment(u, v, 0.355, 0.582, 0.422, 0.648) - 0.013
    d_m = min(d_m1, d_m2)
    a_m = (1.0 - smoothstep(-aa, aa, d_m)) * a_folder
    col = blend(col, (255.0, 255.0, 255.0), a_m)

    # Right side: 3 ordered index bars symbolizing sequence/classification ("图序")
    for idx, (cy_bar, hw_bar) in enumerate([(0.525, 0.095), (0.595, 0.095), (0.665, 0.068)]):
        cx_bar = 0.515 + hw_bar
        d_bar = sd_rounded_box(u, v, cx_bar, cy_bar, hw_bar, 0.018, 0.014)
        a_bar = (1.0 - smoothstep(-aa, aa, d_bar)) * a_folder
        bar_col = (255.0, 255.0, 255.0) if idx < 2 else (191.0, 219.0, 254.0)
        col = blend(col, bar_col, a_bar)

    return (col[0], col[1], col[2], tile_alpha)


def render_rgba(size):
    """Render size x size RGBA bytes with 3x3 supersampling."""
    samples = 3
    out = bytearray(size * size * 4)
    inv_n = 1.0 / (samples * samples)
    for y in range(size):
        for x in range(size):
            sr = sg = sb = sa = 0.0
            for sy in range(samples):
                v = (y + (sy + 0.5) / samples) / size
                for sx in range(samples):
                    u = (x + (sx + 0.5) / samples) / size
                    r, g, b, a = sample_icon(u, v, size)
                    sr += r * a
                    sg += g * a
                    sb += b * a
                    sa += a
            a_avg = sa * inv_n
            idx = (y * size + x) * 4
            if a_avg > 1e-4:
                out[idx] = int(clamp(sr * inv_n / a_avg, 0.0, 255.0) + 0.5)
                out[idx + 1] = int(clamp(sg * inv_n / a_avg, 0.0, 255.0) + 0.5)
                out[idx + 2] = int(clamp(sb * inv_n / a_avg, 0.0, 255.0) + 0.5)
                out[idx + 3] = int(clamp(a_avg * 255.0, 0.0, 255.0) + 0.5)
            else:
                out[idx] = out[idx + 1] = out[idx + 2] = out[idx + 3] = 0
    return bytes(out)


def encode_png(size, rgba):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    raw_rows = bytearray()
    stride = size * 4
    for y in range(size):
        raw_rows.append(0)  # filter type 0
        raw_rows.extend(rgba[y * stride : (y + 1) * stride])
    idat = zlib.compress(bytes(raw_rows), 9)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def encode_ico(png_map):
    sizes = sorted(png_map.keys())
    header = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries = bytearray()
    payload = bytearray()
    for sz in sizes:
        data = png_map[sz]
        w = 0 if sz >= 256 else sz
        h = 0 if sz >= 256 else sz
        entries.extend(struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(data), offset))
        payload.extend(data)
        offset += len(data)
    return header + bytes(entries) + bytes(payload)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    icons_dir = os.path.join(root, "src-tauri", "icons")
    os.makedirs(icons_dir, exist_ok=True)

    targets = {
        "32x32.png": 32,
        "64x64.png": 64,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "icon.png": 512,
    }
    png_cache = {}
    for sz in [16, 24, 32, 48, 64, 128, 256, 512]:
        rgba = render_rgba(sz)
        png_cache[sz] = encode_png(sz, rgba)

    for filename, sz in targets.items():
        with open(os.path.join(icons_dir, filename), "wb") as f:
            f.write(png_cache[sz])

    ico_bytes = encode_ico({sz: png_cache[sz] for sz in [16, 24, 32, 48, 64, 128, 256]})
    with open(os.path.join(icons_dir, "icon.ico"), "wb") as f:
        f.write(ico_bytes)

    print("Generated icons in", icons_dir)


if __name__ == "__main__":
    main()
