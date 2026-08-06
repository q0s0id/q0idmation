"""
Generates installer art for q0editor / q0player.

Produces, in ./assets/ next to this script:

  q0editor.ico        multi-res icon (16/32/48), embedded into q0editor.exe via winres
  q0player.ico        multi-res icon (16/32/48), embedded into q0player.exe via winres
  q0s.ico             multi-res icon (16/32/48) for .q0s files in Explorer
                      (decoded from ./source/q0s.png, if present)
  welcome-editor.bmp  164x314 24-bit BMP, NSIS MUI welcome/finish side panel
  welcome-player.bmp  164x314 24-bit BMP
  header-editor.bmp   150x57  24-bit BMP, NSIS MUI inner-page top banner
  header-player.bmp   150x57  24-bit BMP

No third-party deps - everything is hand-rolled struct.pack + zlib.
Run with:  python build_assets.py
"""

import struct
import zlib
from pathlib import Path

ASSETS = Path(__file__).parent / "assets"
SOURCE = Path(__file__).parent / "source"
ASSETS.mkdir(parents=True, exist_ok=True)

# Palette (stored as BGRA tuples - BMP/ICO native order).
BLACK = (0x14, 0x14, 0x14, 0xFF)
INK = (0x08, 0x08, 0x08, 0xFF)
RED = (0x2E, 0x10, 0xC8, 0xFF)        # primary accent (#C8102E)
RED_DIM = (0x1A, 0x09, 0x6E, 0xFF)    # shadow for red
RED_HOT = (0x55, 0x33, 0xFF, 0xFF)    # highlight (#FF3355)
WHITE = (0xFF, 0xFF, 0xFF, 0xFF)
TRANSPARENT = (0, 0, 0, 0)


# -------------------- BMP / ICO encoding --------------------

def encode_bmp_for_ico(size: int, pix):
    """BITMAPINFOHEADER + bottom-up BGRA rows + AND mask. No file header."""
    header = struct.pack(
        "<IiiHHIIiiII",
        40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0,
    )
    color_data = bytearray()
    for y in range(size - 1, -1, -1):
        row = pix[y]
        for x in range(size):
            b, g, r, a = row[x]
            color_data += bytes((b, g, r, a))
    row_bytes = ((size + 31) // 32) * 4
    and_data = bytearray()
    for y in range(size - 1, -1, -1):
        row = bytearray(row_bytes)
        line = pix[y]
        for x in range(size):
            if line[x][3] == 0:
                row[x // 8] |= 1 << (7 - (x % 8))
        and_data += row
    return bytes(header) + bytes(color_data) + bytes(and_data)


def encode_ico(images):
    out = bytearray()
    out += struct.pack("<HHH", 0, 1, len(images))
    blobs = [encode_bmp_for_ico(size, pix) for size, pix in images]
    data_offset = 6 + 16 * len(images)
    cur = data_offset
    for (size, _), blob in zip(images, blobs):
        w = 0 if size >= 256 else size
        h = 0 if size >= 256 else size
        out += struct.pack(
            "<BBBBHHII",
            w, h, 0, 0, 1, 32, len(blob), cur,
        )
        cur += len(blob)
    for blob in blobs:
        out += blob
    return bytes(out)


def encode_bmp_file(w: int, h: int, pix):
    """24-bit BMP file, BITMAPFILEHEADER + BITMAPINFOHEADER + bottom-up rows."""
    row_bytes = ((w * 3 + 3) // 4) * 4
    pixel_size = row_bytes * h
    file_size = 14 + 40 + pixel_size
    out = bytearray()
    out += struct.pack("<2sIHHI", b"BM", file_size, 0, 0, 14 + 40)
    out += struct.pack(
        "<IiiHHIIiiII",
        40, w, h, 1, 24, 0, pixel_size, 2835, 2835, 0, 0,
    )
    for y in range(h - 1, -1, -1):
        line = pix[y]
        row = bytearray(row_bytes)
        i = 0
        for x in range(w):
            b, g, r, _ = line[x]
            row[i] = b
            row[i + 1] = g
            row[i + 2] = r
            i += 3
        out += row
    return bytes(out)


# -------------------- PNG decoder (8-bit, non-interlaced) --------------------

def decode_png_rgba(path: Path):
    """Minimal PNG -> (width, height, BGRA pixel rows). Supports 8-bit
    color types 0/2/3/4/6, no Adam7 interlace. Enough to consume hand-
    authored source icons; we don't ship a general-purpose decoder."""
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos = 8
    width = height = 0
    bit_depth = color_type = 0
    interlace = 0
    idat = bytearray()
    palette = None
    trns = None
    while pos < len(data):
        length = struct.unpack(">I", data[pos:pos + 4])[0]
        pos += 4
        ctype = data[pos:pos + 4]
        pos += 4
        chunk = data[pos:pos + length]
        pos += length + 4  # skip CRC
        if ctype == b"IHDR":
            width, height = struct.unpack(">II", chunk[:8])
            bit_depth = chunk[8]
            color_type = chunk[9]
            interlace = chunk[12]
        elif ctype == b"IDAT":
            idat += chunk
        elif ctype == b"PLTE":
            palette = chunk
        elif ctype == b"tRNS":
            trns = chunk
        elif ctype == b"IEND":
            break

    if bit_depth != 8:
        raise ValueError(f"{path}: only 8-bit PNG supported (got {bit_depth})")
    if interlace != 0:
        raise ValueError(f"{path}: Adam7 interlaced PNG not supported")

    bpp_map = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}
    if color_type not in bpp_map:
        raise ValueError(f"{path}: color type {color_type} not supported")
    bpp = bpp_map[color_type]

    raw = zlib.decompress(bytes(idat))
    stride = width * bpp
    rows = []
    prev = bytes(stride)
    p = 0
    for _ in range(height):
        ftype = raw[p]
        p += 1
        scan = bytearray(raw[p:p + stride])
        p += stride
        if ftype == 0:
            pass
        elif ftype == 1:  # Sub
            for i in range(bpp, stride):
                scan[i] = (scan[i] + scan[i - bpp]) & 0xFF
        elif ftype == 2:  # Up
            for i in range(stride):
                scan[i] = (scan[i] + prev[i]) & 0xFF
        elif ftype == 3:  # Average
            for i in range(stride):
                left = scan[i - bpp] if i >= bpp else 0
                up = prev[i]
                scan[i] = (scan[i] + (left + up) // 2) & 0xFF
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = scan[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                pp = a + b - c
                pa = abs(pp - a)
                pb = abs(pp - b)
                pc = abs(pp - c)
                if pa <= pb and pa <= pc:
                    pred = a
                elif pb <= pc:
                    pred = b
                else:
                    pred = c
                scan[i] = (scan[i] + pred) & 0xFF
        else:
            raise ValueError(f"{path}: bad filter {ftype}")
        rows.append(bytes(scan))
        prev = scan

    out = []
    for y in range(height):
        scan = rows[y]
        line = []
        for x in range(width):
            if color_type == 6:
                r = scan[x * 4]
                g = scan[x * 4 + 1]
                b = scan[x * 4 + 2]
                a = scan[x * 4 + 3]
            elif color_type == 2:
                r = scan[x * 3]
                g = scan[x * 3 + 1]
                b = scan[x * 3 + 2]
                a = 255
            elif color_type == 4:
                r = g = b = scan[x * 2]
                a = scan[x * 2 + 1]
            elif color_type == 0:
                r = g = b = scan[x]
                a = 255
            else:  # palette
                idx = scan[x]
                r = palette[idx * 3]
                g = palette[idx * 3 + 1]
                b = palette[idx * 3 + 2]
                a = trns[idx] if trns and idx < len(trns) else 255
            line.append((b, g, r, a))
        out.append(line)
    return width, height, out


def resample_box(pix, src_w, src_h, dst_w, dst_h):
    """Box-average downsample with proper alpha handling. For upsample
    falls back to nearest neighbor."""
    if dst_w >= src_w and dst_h >= src_h:
        out = []
        for y in range(dst_h):
            sy = min(src_h - 1, y * src_h // dst_h)
            row = []
            for x in range(dst_w):
                sx = min(src_w - 1, x * src_w // dst_w)
                row.append(pix[sy][sx])
            out.append(row)
        return out

    out = []
    for y in range(dst_h):
        y0 = y * src_h // dst_h
        y1 = max(y0 + 1, (y + 1) * src_h // dst_h)
        row = []
        for x in range(dst_w):
            x0 = x * src_w // dst_w
            x1 = max(x0 + 1, (x + 1) * src_w // dst_w)
            r_acc = g_acc = b_acc = a_acc = 0
            count = 0
            for yy in range(y0, y1):
                for xx in range(x0, x1):
                    bb, gg, rr, aa = pix[yy][xx]
                    # Pre-multiply by alpha so transparent pixels don't
                    # bleed their (often garbage) RGB into the average.
                    r_acc += rr * aa
                    g_acc += gg * aa
                    b_acc += bb * aa
                    a_acc += aa
                    count += 1
            if a_acc > 0:
                r = r_acc // a_acc
                g = g_acc // a_acc
                b = b_acc // a_acc
            else:
                r = g = b = 0
            a = a_acc // count if count else 0
            row.append((b, g, r, a))
        out.append(row)
    return out


# -------------------- drawing primitives --------------------

def fill_rect(pix, x0, y0, x1, y1, color):
    h = len(pix)
    w = len(pix[0])
    for y in range(max(0, y0), min(h, y1)):
        row = pix[y]
        for x in range(max(0, x0), min(w, x1)):
            row[x] = color


def stroke_rect(pix, x0, y0, x1, y1, color):
    fill_rect(pix, x0, y0, x1, y0 + 1, color)
    fill_rect(pix, x0, y1 - 1, x1, y1, color)
    fill_rect(pix, x0, y0, x0 + 1, y1, color)
    fill_rect(pix, x1 - 1, y0, x1, y1, color)


def draw_q_ring(pix, cx, cy, inner, outer, color):
    h = len(pix)
    w = len(pix[0])
    y0 = max(0, int(cy - outer) - 1)
    y1 = min(h, int(cy + outer) + 2)
    x0 = max(0, int(cx - outer) - 1)
    x1 = min(w, int(cx + outer) + 2)
    inner2 = inner * inner
    outer2 = outer * outer
    for y in range(y0, y1):
        dy = y - cy
        for x in range(x0, x1):
            dx = x - cx
            d2 = dx * dx + dy * dy
            if inner2 <= d2 <= outer2:
                pix[y][x] = color


def draw_q_tail(pix, cx, cy, length, thickness, color):
    h = len(pix)
    w = len(pix[0])
    for t in range(length):
        for k in range(thickness):
            x = int(cx + t)
            y = int(cy + t + k)
            if 0 <= x < w and 0 <= y < h:
                pix[y][x] = color


# -------------------- icon design --------------------

def make_icon(size: int, kind: str):
    """Black chip with red border and a red 'Q' monogram. A small white
    indicator in the upper-left distinguishes editor (square) from player
    (triangle)."""
    pix = [[TRANSPARENT] * size for _ in range(size)]

    margin = max(1, size // 16)
    chip_x0, chip_y0 = margin, margin
    chip_x1, chip_y1 = size - margin, size - margin

    fill_rect(pix, chip_x0, chip_y0, chip_x1, chip_y1, INK)
    stroke_rect(pix, chip_x0, chip_y0, chip_x1, chip_y1, RED)

    inset = chip_x1 - chip_x0
    cx = (chip_x0 + chip_x1) / 2
    cy = (chip_y0 + chip_y1) / 2 - size * 0.025
    inner_r = inset * 0.22
    outer_r = inset * 0.36
    draw_q_ring(pix, cx, cy, inner_r, outer_r, RED)

    tail_len = max(2, int(inset * 0.18))
    tail_thick = max(1, size // 16)
    draw_q_tail(
        pix,
        cx + inset * 0.16,
        cy + inset * 0.16,
        tail_len,
        tail_thick,
        RED,
    )

    # Corner indicator
    pad = margin + max(1, size // 24)
    if kind == "editor":
        s = max(2, size // 9)
        fill_rect(pix, pad, pad, pad + s, pad + s, WHITE)
    else:  # player
        s = max(3, size // 7)
        for dy in range(s):
            for dx in range(s - dy):
                pix[pad + dy][pad + dx] = WHITE

    return pix


# -------------------- welcome / header BMP --------------------

def _scanline_dim(pix, period=4, drop=22):
    h = len(pix)
    w = len(pix[0])
    for y in range(0, h, period):
        for x in range(w):
            b, g, r, a = pix[y][x]
            pix[y][x] = (
                max(0, b - drop),
                max(0, g - drop),
                max(0, r - drop),
                a,
            )


def make_welcome(w: int, h: int, kind: str):
    """Tall left-side strip: ink-black with a soft red glow from the top,
    a big Q logo near the top, a thin red accent line near the bottom,
    and CRT scanlines for old-school feel."""
    pix = [[INK] * w for _ in range(h)]

    # Soft top-center red glow
    for y in range(h):
        for x in range(w):
            dx = (x - w / 2) / (w * 0.7)
            dy = (y - 8) / (h * 0.6)
            d = (dx * dx + dy * dy) ** 0.5
            t = max(0.0, 1.0 - d)
            t = t * t * 0.55
            b = int(INK[0] + (RED[0] - INK[0]) * t)
            g = int(INK[1] + (RED[1] - INK[1]) * t)
            r = int(INK[2] + (RED[2] - INK[2]) * t)
            pix[y][x] = (b, g, r, 0xFF)

    # Big Q logo
    qsize = 96
    cx = w / 2
    cy = 18 + qsize / 2
    inner_r = qsize * 0.26
    outer_r = qsize * 0.40
    draw_q_ring(pix, cx, cy, inner_r, outer_r, RED)
    draw_q_tail(pix, cx + qsize * 0.18, cy + qsize * 0.18, 18, 3, RED)

    # Q outer rim - one-pixel hot highlight on top-left, dim on bottom-right
    for y in range(h):
        for x in range(w):
            dx = x - cx
            dy = y - cy
            d2 = dx * dx + dy * dy
            r2 = (outer_r + 1) * (outer_r + 1)
            r3 = outer_r * outer_r
            if r3 <= d2 <= r2:
                # top-left of the ring -> highlight
                if dx + dy < -outer_r * 0.4:
                    pix[y][x] = RED_HOT

    # Project name in pixel-strip style under the logo
    name_y = int(cy + outer_r) + 18
    fill_rect(pix, 16, name_y, w - 16, name_y + 1, RED_DIM)
    fill_rect(pix, 16, name_y + 8, w - 16, name_y + 9, RED_DIM)
    # tick marks between the lines
    tick_y0 = name_y + 2
    tick_y1 = name_y + 7
    for x in range(20, w - 20, 6):
        fill_rect(pix, x, tick_y0, x + 2, tick_y1, RED)

    # Subtle barcode strip near the bottom (faux dataline)
    bar_y = h - 36
    fill_rect(pix, 14, bar_y, w - 14, bar_y + 9, INK)
    stroke_rect(pix, 14, bar_y, w - 14, bar_y + 9, RED_DIM)
    rng = 0x12345
    x = 18
    while x < w - 18:
        rng = (rng * 1103515245 + 12345) & 0x7FFFFFFF
        bw = 1 + (rng >> 8) % 4
        if (rng >> 4) & 1:
            fill_rect(pix, x, bar_y + 2, x + bw, bar_y + 7, RED)
        x += bw + 1

    # Bottom-edge red rule
    fill_rect(pix, 0, h - 4, w, h - 3, RED_DIM)
    fill_rect(pix, 0, h - 3, w, h - 2, RED)

    # Editor vs player badge in the lower area
    badge_y = h - 70
    badge_x0 = 18
    badge_x1 = w - 18
    fill_rect(pix, badge_x0, badge_y, badge_x1, badge_y + 18, INK)
    stroke_rect(pix, badge_x0, badge_y, badge_x1, badge_y + 18, RED)
    if kind == "editor":
        # square pip + 4 little lines = "edit"
        fill_rect(pix, badge_x0 + 5, badge_y + 5, badge_x0 + 11, badge_y + 11, WHITE)
        for i in range(4):
            yy = badge_y + 4 + i * 3
            fill_rect(pix, badge_x0 + 16, yy, badge_x0 + 16 + 22, yy + 1, RED_HOT)
    else:
        # play triangle + waveform = "play"
        for dy in range(10):
            for dx in range(10 - dy):
                pix[badge_y + 4 + dy][badge_x0 + 5 + dx] = WHITE
        for i, hbar in enumerate([2, 5, 3, 7, 4, 6, 2, 5]):
            xx = badge_x0 + 20 + i * 3
            yy_top = badge_y + 9 - hbar // 2
            yy_bot = yy_top + hbar
            fill_rect(pix, xx, yy_top, xx + 2, yy_bot, RED_HOT)

    _scanline_dim(pix, period=3, drop=10)
    return pix


def make_header(w: int, h: int, kind: str):
    """Short 150x57 banner: dark with a small Q badge on the left and the
    kind label on the right (rendered as a stylized stripe rather than text;
    NSIS draws real text on top, so we just supply texture)."""
    pix = [[INK] * w for _ in range(h)]

    # subtle horizontal red glow line behind the badge
    for y in range(h):
        t = 1.0 - abs(y - h / 2) / (h / 2)
        t = max(0.0, t) * 0.18
        for x in range(w):
            tx = max(0.0, 1.0 - x / (w * 0.7))
            tt = t * tx
            b = int(INK[0] + (RED[0] - INK[0]) * tt)
            g = int(INK[1] + (RED[1] - INK[1]) * tt)
            r = int(INK[2] + (RED[2] - INK[2]) * tt)
            pix[y][x] = (b, g, r, 0xFF)

    # Mini Q badge on the left
    qsize = 38
    qx, qy = 8, (h - qsize) // 2
    cx = qx + qsize / 2
    cy = qy + qsize / 2
    inner_r = qsize * 0.26
    outer_r = qsize * 0.40
    draw_q_ring(pix, cx, cy, inner_r, outer_r, RED)
    draw_q_tail(pix, cx + qsize * 0.18, cy + qsize * 0.18, 7, 2, RED)

    # Right side: data-stream stripes
    stripes_x0 = qx + qsize + 8
    stripes_x1 = w - 4
    for i, hbar in enumerate([3, 6, 2, 8, 4, 7, 2, 5, 3, 6, 4, 8, 2, 5]):
        if i * 4 + stripes_x0 + 2 > stripes_x1:
            break
        xx = stripes_x0 + i * 4
        yy_top = h // 2 - hbar // 2
        yy_bot = yy_top + hbar
        col = RED if i % 3 != 2 else RED_HOT
        fill_rect(pix, xx, yy_top, xx + 2, yy_bot, col)

    # Bottom red rule
    fill_rect(pix, 0, h - 2, w, h - 1, RED_DIM)
    fill_rect(pix, 0, h - 1, w, h, RED)

    # Indicator pip in the upper-right corner
    if kind == "editor":
        fill_rect(pix, w - 12, 4, w - 6, 10, WHITE)
    else:
        for dy in range(7):
            for dx in range(7 - dy):
                pix[4 + dy][w - 13 + dx] = WHITE

    return pix


# -------------------- entry point --------------------

def build_q0s_icon_from_png():
    """Encode source/q0s.png into assets/q0s.ico at 16/32/48. This is the
    icon Explorer shows for .q0s files; the player exe has its own icon."""
    src = SOURCE / "q0s.png"
    if not src.is_file():
        print(f"  skip q0s.ico (no {src.relative_to(Path(__file__).parent)})")
        return
    sw, sh, pix = decode_png_rgba(src)
    sizes = [16, 32, 48]
    imgs = [(s, resample_box(pix, sw, sh, s, s)) for s in sizes]
    path = ASSETS / "q0s.ico"
    path.write_bytes(encode_ico(imgs))
    print(f"  {path.name}  ({len(sizes)} sizes, from {sw}x{sh} png)")


def main():
    print("building installer assets ->", ASSETS)
    sizes = [16, 32, 48]
    for kind in ("editor", "player"):
        imgs = [(s, make_icon(s, kind)) for s in sizes]
        path = ASSETS / f"q0{kind}.ico"
        path.write_bytes(encode_ico(imgs))
        print(f"  {path.name}  ({len(sizes)} sizes)")

    build_q0s_icon_from_png()

    for kind in ("editor", "player"):
        pix = make_welcome(164, 314, kind)
        path = ASSETS / f"welcome-{kind}.bmp"
        path.write_bytes(encode_bmp_file(164, 314, pix))
        print(f"  {path.name}")

    for kind in ("editor", "player"):
        pix = make_header(150, 57, kind)
        path = ASSETS / f"header-{kind}.bmp"
        path.write_bytes(encode_bmp_file(150, 57, pix))
        print(f"  {path.name}")

    print("done.")


if __name__ == "__main__":
    main()
