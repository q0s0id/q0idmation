"""
Draws every q0idmation icon: the apps (q0editor, q0player, CanRipper) and
the file types (.q0s, .q0v, .q1s, .q0l).

The family look stays the old one: a black chip with a red border and a red
Q. What is new is how they are made. Shapes are drawn 8x too big and scaled
down, so their edges are smooth; then the dirt goes on at the real size,
pixel by pixel: grain, scanlines, grime, scratches, a worn border and a Q
printed slightly off its register. Small sizes get less dirt so they stay
readable. The noise is seeded, so the same script makes the same files.

Writes:
  assets/<name>.ico      16, 20, 24, 32, 40, 48, 64 (bitmap) and 128, 256 (png)
  ../android/{app,player}/src/main/res/mipmap-*/ic_launcher.png
  ../out/icons-preview.png   every icon at every size, to look at

Needs Pillow and numpy:  python build_icons.py
"""

import io
import random
import struct
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).parent
ASSETS = HERE / "assets"
ROOT = HERE.parent

ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
ANDROID = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}


def rgb(hex_colour, alpha=255):
    h = hex_colour.lstrip("#")
    return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16), alpha)


RED = rgb("#c8102e")
RED_HOT = rgb("#ff3a52")
RED_DEEP = rgb("#7a0718")
RED_GHOST = rgb("#5a0412")
INK_TOP = rgb("#241719")
INK_BOTTOM = rgb("#080506")
BONE = rgb("#efe5d6")
BONE_DIM = rgb("#a99d8c")
SOOT = rgb("#120a0b")

# label strip colours per file type: (background, text)
LABELS = {
    "q0s": (rgb("#c8102e"), rgb("#f6ece0")),
    "q0v": (rgb("#d98f1c"), rgb("#1a0f08")),
    "q1s": (rgb("#e6dccd"), rgb("#1a1012")),
    "q0l": (rgb("#5a9e3e"), rgb("#0c1408")),
}

# 5-row pixel font for the labels; Q and S are drawn so they can't pass
# for 9 and 5
FONT = {
    "Q": ["0110", "1001", "1001", "1011", "0111"],
    "0": ["111", "101", "101", "101", "111"],
    "1": ["010", "110", "010", "010", "111"],
    "S": ["011", "100", "010", "001", "110"],
    "V": ["101", "101", "101", "101", "010"],
    "L": ["100", "100", "100", "100", "111"],
}


# ----------------------------------------------------------------- drawing --

class Canvas:
    """An oversized square to draw in unit coordinates (0..1)."""

    def __init__(self, size):
        self.size = size
        self.ss = 8 if size <= 64 else 4
        self.n = size * self.ss
        self.img = Image.new("RGBA", (self.n, self.n), (0, 0, 0, 0))

    def px(self, v):
        return v * self.n

    def pts(self, points):
        return [(self.px(x), self.px(y)) for x, y in points]

    def mask(self):
        return Image.new("L", (self.n, self.n), 0)

    def put(self, layer):
        self.img.alpha_composite(layer)

    def fill(self, mask, colour):
        layer = Image.new("RGBA", (self.n, self.n), colour)
        layer.putalpha(Image.fromarray(
            (np.asarray(mask, dtype=np.float32) * (colour[3] / 255.0)).astype(np.uint8)))
        self.put(layer)

    def gradient(self, mask, top, bottom, angle_down=True):
        """Fill `mask` with a vertical (or diagonal) blend from top to bottom."""
        n = self.n
        ys, xs = np.mgrid[0:n, 0:n].astype(np.float32) / n
        t = ys if angle_down else (xs + ys) / 2
        t = np.clip(t, 0, 1)[..., None]
        a = np.array(top, np.float32)
        b = np.array(bottom, np.float32)
        colour = a + (b - a) * t
        colour[..., 3] *= np.asarray(mask, np.float32) / 255.0
        self.put(Image.fromarray(colour.astype(np.uint8), "RGBA"))

    def shrink(self):
        """Down to the real size, averaging in premultiplied alpha so edges
        don't pick up dark fringes."""
        a = np.asarray(self.img, np.float32) / 255.0
        a[..., :3] *= a[..., 3:4]
        s = self.ss
        n = self.size
        a = a.reshape(n, s, n, s, 4).mean(axis=(1, 3))
        alpha = a[..., 3:4]
        a[..., :3] = np.where(alpha > 1e-4, a[..., :3] / np.maximum(alpha, 1e-4), 0)
        return np.clip(a * 255.0, 0, 255)


def polygon_mask(c, points):
    m = c.mask()
    ImageDraw.Draw(m).polygon(c.pts(points), fill=255)
    return m


def ring_mask(c, cx, cy, outer, inner):
    m = c.mask()
    d = ImageDraw.Draw(m)
    d.ellipse([c.px(cx - outer), c.px(cy - outer), c.px(cx + outer), c.px(cy + outer)], fill=255)
    d.ellipse([c.px(cx - inner), c.px(cy - inner), c.px(cx + inner), c.px(cy + inner)], fill=0)
    return m


def disc_mask(c, cx, cy, r):
    m = c.mask()
    ImageDraw.Draw(m).ellipse([c.px(cx - r), c.px(cy - r), c.px(cx + r), c.px(cy + r)], fill=255)
    return m


def bar_mask(c, x0, y0, x1, y1, half_width):
    """A thick straight stroke with square ends."""
    dx, dy = x1 - x0, y1 - y0
    length = (dx * dx + dy * dy) ** 0.5
    nx, ny = -dy / length * half_width, dx / length * half_width
    return polygon_mask(c, [(x0 + nx, y0 + ny), (x1 + nx, y1 + ny), (x1 - nx, y1 - ny), (x0 - nx, y0 - ny)])


def combine(*masks, op="or"):
    arrays = [np.asarray(m, np.float32) for m in masks]
    out = arrays[0]
    for other in arrays[1:]:
        out = np.maximum(out, other) if op == "or" else out * other / 255.0
    return Image.fromarray(out.astype(np.uint8), "L")


def subtract(mask, cut):
    a = np.asarray(mask, np.float32) * (1 - np.asarray(cut, np.float32) / 255.0)
    return Image.fromarray(a.astype(np.uint8), "L")


def outline_of(c, mask, width_units):
    """The band just inside a shape's edge."""
    width = max(1, int(round(c.px(width_units))))
    # erode by blurring and keeping only what stays fully inside: a box blur
    # costs the same at any width, a min filter grows with its square
    blurred = np.asarray(mask.filter(ImageFilter.BoxBlur(width)), np.float32)
    inner = Image.fromarray(np.where(blurred >= 250, 255, 0).astype(np.uint8), "L")
    return subtract(mask, inner)


# ------------------------------------------------------------------- parts --

MARGIN = 0.055


def chip_points(fold):
    m = MARGIN
    if fold <= 0:
        return [(m, m), (1 - m, m), (1 - m, 1 - m), (m, 1 - m)]
    return [(m, m), (1 - m - fold, m), (1 - m, m + fold), (1 - m, 1 - m), (m, 1 - m)]


def draw_chip(c, rng, fold, dirt):
    size = c.size
    body = polygon_mask(c, chip_points(fold))
    if size >= 32:
        # a drop shadow, as desktop icons had
        shadow = body.transform(body.size, Image.AFFINE, (1, 0, -c.px(0.028), 0, 1, -c.px(0.036)))
        shadow = shadow.filter(ImageFilter.GaussianBlur(c.px(0.02)))
        c.fill(shadow, (0, 0, 0, 120))
    c.gradient(body, INK_TOP, INK_BOTTOM)

    if dirt >= 2:
        # soft grime blotches and a vignette
        grime = c.mask()
        d = ImageDraw.Draw(grime)
        for _ in range(5):
            x, y = rng.uniform(0.15, 0.85), rng.uniform(0.15, 0.85)
            r = rng.uniform(0.05, 0.14)
            d.ellipse([c.px(x - r), c.px(y - r * 0.7), c.px(x + r), c.px(y + r * 0.7)], fill=rng.randint(60, 140))
        grime = grime.filter(ImageFilter.GaussianBlur(c.px(0.04)))
        c.fill(combine(grime, body, op="and"), (46, 22, 14, 90))
        vignette = Image.fromarray(vignette_array(c.n), "L")
        c.fill(combine(vignette, body, op="and"), (0, 0, 0, 150))
    if dirt >= 3:
        # a few light scratches across the plastic
        scratches = c.mask()
        d = ImageDraw.Draw(scratches)
        for _ in range(4):
            x, y = rng.uniform(0.1, 0.9), rng.uniform(0.1, 0.9)
            ang = rng.uniform(-0.6, 0.6) + (np.pi / 4 if rng.random() < 0.5 else -np.pi / 5)
            ln = rng.uniform(0.12, 0.3)
            d.line([c.px(x), c.px(y), c.px(x + np.cos(ang) * ln), c.px(y + np.sin(ang) * ln)],
                   fill=rng.randint(90, 170), width=max(1, int(c.px(0.006))))
        c.fill(combine(scratches, body, op="and"), (210, 190, 180, 70))

    # the red border with a bevel: lighter where the light falls, deeper below
    border_w = max(0.05, 1.15 / size)
    border = outline_of(c, body, border_w)
    if dirt >= 2:
        wear = c.mask()
        d = ImageDraw.Draw(wear)
        for _ in range(9 if size >= 64 else 5):
            x, y = rng.uniform(0.05, 0.95), rng.uniform(0.05, 0.95)
            r = rng.uniform(0.012, 0.03)
            d.ellipse([c.px(x - r), c.px(y - r), c.px(x + r), c.px(y + r)], fill=150)
        border = subtract(border, combine(wear, border, op="and"))
    c.gradient(border, RED_HOT, RED_DEEP, angle_down=False)
    if size >= 24:
        inner_glow = outline_of(c, subtract(body, border.filter(ImageFilter.MaxFilter(3))), 0.012)
        c.fill(inner_glow, (255, 120, 120, 40))

    if fold > 0:
        m = MARGIN
        corner = [(1 - m - fold, m), (1 - m - fold, m + fold), (1 - m, m + fold)]
        paper = polygon_mask(c, corner)
        c.gradient(paper, BONE, BONE_DIM, angle_down=False)
        c.fill(outline_of(c, paper, border_w * 0.8), RED_DEEP)
        crease = bar_mask(c, 1 - m - fold, m, 1 - m, m + fold, max(0.012, 0.6 / size))
        c.fill(crease, RED)
    return body


def vignette_array(n):
    ys, xs = np.mgrid[0:n, 0:n].astype(np.float32) / n - 0.5
    d = np.sqrt(xs * xs + ys * ys) / 0.7
    return (np.clip((d - 0.45) / 0.55, 0, 1) ** 1.6 * 255).astype(np.uint8)


def draw_q(c, cx, cy, scale, dirt):
    outer, inner = 0.29 * scale, 0.165 * scale
    ring = ring_mask(c, cx, cy, outer, inner)
    ang = np.pi / 4
    tail = bar_mask(c, cx + np.cos(ang) * 0.2 * scale, cy + np.sin(ang) * 0.2 * scale,
                    cx + np.cos(ang) * 0.415 * scale, cy + np.sin(ang) * 0.415 * scale, 0.052 * scale)
    q = combine(ring, tail)
    if c.size >= 32:
        # printed a little off: a dark red ghost peeking out below and right
        ghost = q.transform(q.size, Image.AFFINE, (1, 0, -c.px(0.016), 0, 1, -c.px(0.018)))
        c.fill(ghost, RED_GHOST)
    c.gradient(q, RED_HOT, RED_DEEP, angle_down=False)
    if c.size >= 24:
        # a glossy highlight on the top left of the ring
        shine = c.mask()
        r = (outer + inner) / 2
        ImageDraw.Draw(shine).arc(
            [c.px(cx - r), c.px(cy - r), c.px(cx + r), c.px(cy + r)], 195, 265,
            fill=255, width=max(1, int(c.px((outer - inner) * 0.28))))
        shine = shine.filter(ImageFilter.GaussianBlur(c.px(0.008)))
        c.fill(combine(shine, ring, op="and"), (255, 225, 220, 120))
        # and the inside edge in shadow
        c.fill(outline_of(c, disc_mask(c, cx, cy, inner + 0.02 * scale), 0.02 * scale), (20, 0, 4, 120))
    return q


def glyph(c, kind, cx, cy, scale):
    """What sits in the Q's hole: the file type at a glance."""
    r = 0.098 * scale
    if kind == "q0s":  # the movie: the old diamond
        return polygon_mask(c, [(cx, cy - r), (cx + r, cy), (cx, cy + r), (cx - r, cy)])
    if kind == "q0v":  # video: play
        return polygon_mask(c, [(cx - r * 0.7, cy - r), (cx + r, cy), (cx - r * 0.7, cy + r)])
    if kind == "q1s":  # the project you draw in: a pencil (its tip: pencil_tip)
        w = r * 0.36
        ax, ay = cx - r * 0.3, cy + r * 0.3  # where the wood starts
        return bar_mask(c, ax, ay, cx + r * 0.95, cy - r * 0.95, w)
    if kind == "q0l":  # code: a prompt, > and _
        chevron = combine(bar_mask(c, cx - r * 0.95, cy - r * 0.75, cx - r * 0.05, cy, r * 0.24),
                          bar_mask(c, cx - r * 0.05, cy, cx - r * 0.95, cy + r * 0.75, r * 0.24))
        under = bar_mask(c, cx + r * 0.15, cy + r * 0.68, cx + r * 1.0, cy + r * 0.68, r * 0.2)
        return combine(chevron, under)
    raise ValueError(kind)


def pencil_tip(c, cx, cy, scale):
    """The sharpened end of the .q1s pencil: bare wood and a graphite point."""
    r = 0.098 * scale
    w = r * 0.36
    ax, ay = cx - r * 0.3, cy + r * 0.3
    px, py = cx - r * 1.0, cy + r * 1.0
    wood = polygon_mask(c, [(ax - w * 0.71, ay - w * 0.71), (ax + w * 0.71, ay + w * 0.71), (px, py)])
    c.gradient(wood, BONE, BONE_DIM, angle_down=False)
    k = 0.42
    point = polygon_mask(c, [(px + (ax - w * 0.71 - px) * k, py + (ay - w * 0.71 - py) * k),
                             (px + (ax + w * 0.71 - px) * k, py + (ay + w * 0.71 - py) * k), (px, py)])
    c.fill(point, SOOT)


def sticker(c, shape, x, y, s):
    """The corner marks on the app icons: bone-white with a dark edge."""
    if shape == "square":
        m = polygon_mask(c, [(x, y), (x + s, y), (x + s, y + s), (x, y + s)])
    else:
        m = polygon_mask(c, [(x, y), (x + s * 1.05, y), (x, y + s * 1.05)])
    drop = m.transform(m.size, Image.AFFINE, (1, 0, -c.px(0.012), 0, 1, -c.px(0.014)))
    c.fill(drop, (0, 0, 0, 160))
    c.gradient(m, BONE, BONE_DIM)
    if c.size >= 32:
        c.fill(outline_of(c, m, 0.012), SOOT)


GOLD = rgb("#ffd25a")
EMBER = rgb("#ff6a14")
SPARK = rgb("#fff6c8")


def glow(c, mask, colour, radius):
    """Light spilling around a shape: its mask, blurred, laid on top."""
    c.fill(mask.filter(ImageFilter.GaussianBlur(c.px(radius))), colour)


def star_mask(c, cx, cy, r, spin=0.0, points=5, inner=0.45):
    pts = []
    for k in range(points * 2):
        a = spin - np.pi / 2 + k * np.pi / points
        rr = r if k % 2 == 0 else r * inner
        pts.append((cx + np.cos(a) * rr, cy + np.sin(a) * rr))
    return polygon_mask(c, pts)


def sliver_mask(c, x0, y0, x1, y1, width):
    """A claw slash: thick in the middle, sharp at both ends."""
    dx, dy = x1 - x0, y1 - y0
    length = (dx * dx + dy * dy) ** 0.5
    nx, ny = -dy / length * width, dx / length * width
    mx, my = (x0 + x1) / 2, (y0 + y1) / 2
    return polygon_mask(c, [(x0, y0), (mx + nx, my + ny), (x1, y1), (mx - nx, my - ny)])


def draw_can(c, rng, dirt):
    """CanRipper: a tin can clawed open. Light pours out of it, the lid is
    blown off and what was inside bursts out past the frame."""
    size = c.size
    big = size >= 48
    mid = size >= 24
    body = polygon_mask(c, chip_points(0))

    # behind it all: a red burst and rays, like a sticker shouting NEW!
    cx0, cy0 = 0.5, 0.5
    rays = c.mask()
    d = ImageDraw.Draw(rays)
    count = 16
    for k in range(count):
        if k % 2:
            continue
        a0 = k * 2 * np.pi / count + 0.1
        a1 = a0 + 2 * np.pi / count
        d.polygon([(c.px(cx0), c.px(cy0)),
                   (c.px(cx0 + np.cos(a0) * 1.0), c.px(cy0 + np.sin(a0) * 1.0)),
                   (c.px(cx0 + np.cos(a1) * 1.0), c.px(cy0 + np.sin(a1) * 1.0))], fill=255)
    if size > 20:
        c.fill(combine(rays, body, op="and"), (110, 6, 20, 95))
        glow(c, disc_mask(c, 0.5, 0.52, 0.26), (220, 34, 26, 105), 0.12)
    else:
        glow(c, disc_mask(c, 0.5, 0.55, 0.22), (200, 30, 24, 70), 0.1)

    # the can is drawn upright on its own layer, then tipped over
    L = Canvas(size)
    x0, x1 = (0.34, 0.66) if size > 20 else (0.3, 0.7)
    top, bottom = (0.40, 0.83) if size > 20 else (0.3, 0.84)
    rx, ry = (x1 - x0) / 2, 0.05
    cx = (x0 + x1) / 2
    can = combine(polygon_mask(L, [(x0, top), (x1, top), (x1, bottom), (x0, bottom)]),
                  ellipse_mask(L, cx, bottom, rx, ry))
    L.fill(can.transform(can.size, Image.AFFINE, (1, 0, -L.px(0.025), 0, 1, -L.px(0.03))), (0, 0, 0, 170))
    metal = metal_array(L.n, x0, x1)
    metal[..., 3] *= np.asarray(can, np.float32) / 255.0
    L.put(Image.fromarray(metal.astype(np.uint8), "RGBA"))
    # rim bands at the top and bottom, as on a real tin
    for y in (top + 0.035, bottom - 0.03):
        L.fill(combine(can, bar_mask(L, 0, y, 1, y, 0.007), op="and"), (70, 64, 58, 200))
    # the red label with a lightning bolt
    label_top, label_bottom = 0.5, 0.72
    lab = combine(can, polygon_mask(L, [(0, label_top), (1, label_top), (1, label_bottom), (0, label_bottom)]), op="and")
    L.gradient(lab, RED_HOT, RED_DEEP, angle_down=False)
    for y in (label_top, label_bottom):
        L.fill(combine(can, bar_mask(L, 0, y, 1, y, 0.007), op="and"), (50, 0, 8, 220))
    # torn open: the dark inside glowing gold, and a jagged rim of teeth
    inside = ellipse_mask(L, cx, top, rx * 0.95, ry)
    L.gradient(inside, GOLD, EMBER)
    teeth = []
    steps = 8
    for i in range(steps + 1):
        t = i / steps
        lift = 0.05 if i % 2 else -0.004
        teeth.append((x0 + (x1 - x0) * t, top - (lift if 0 < i < steps else 0)))
    teeth += [(x1, top + 0.014), (x0, top + 0.014)]
    L.gradient(polygon_mask(L, teeth), rgb("#f4eee4"), rgb("#8c847a"))
    # three claw marks ripped through the side, white hot at the edges
    slashes = []
    n_slash = 3 if size >= 20 else 1
    for i in range(n_slash):
        off = (i - (n_slash - 1) / 2) * 0.09
        slashes.append(sliver_mask(L, cx - 0.2 + off, 0.45, cx + 0.12 + off, 0.8, 0.032 if size >= 20 else 0.05))
    torn = combine(*slashes)
    torn = combine(torn, can, op="and")
    if mid:
        glow(L, torn, (255, 120, 30, 230), 0.02)
    L.fill(torn, SOOT)
    if big:
        inner_edge = outline_of(L, torn, 0.008)
        L.fill(inner_edge, (255, 190, 90, 220))
    if size >= 32:
        L.fill(outline_of(L, can, 0.01), rgb("#2a2420"))
    tipped = L.img.rotate(14, resample=Image.BICUBIC, center=(L.px(0.5), L.px(0.62)))
    c.put(tipped)

    # where the can tipped to: its opening's centre, now up and to the left
    a = np.radians(-14)
    ox = 0.5 + (cx - 0.5) * np.cos(a) - (top - 0.62) * np.sin(a)
    oy = 0.62 + (cx - 0.5) * np.sin(a) + (top - 0.62) * np.cos(a)

    if mid:
        # light pouring out of the open can
        beams = c.mask()
        d = ImageDraw.Draw(beams)
        for spread in (-0.5, 0.0, 0.5):
            ang = -np.pi / 2 - 0.25 + spread * 0.5
            far = 0.6
            w = 0.07
            px_, py_ = ox + np.cos(ang) * far, oy + np.sin(ang) * far
            nx, ny = -np.sin(ang) * w, np.cos(ang) * w
            d.polygon([(c.px(ox - 0.06), c.px(oy)), (c.px(ox + 0.06), c.px(oy)),
                       (c.px(px_ + nx), c.px(py_ + ny)), (c.px(px_ - nx), c.px(py_ - ny))], fill=150)
        beams = beams.filter(ImageFilter.GaussianBlur(c.px(0.03)))
        c.fill(combine(beams, body, op="and"), (255, 214, 110, 150))

    if mid:
        # the lid, blown off up and to the left, spinning, with motion lines
        lx, ly = 0.25, 0.22
        if big:
            for k, (dx, dy) in enumerate(((0.07, 0.07), (0.1, 0.035), (0.04, 0.1))):
                c.fill(bar_mask(c, lx + dx, ly + dy, lx + dx + 0.12, ly + dy + 0.12, 0.009),
                       (255, 230, 200, 110 - k * 20))
        lid = rotated_ellipse_mask(c, lx, ly, 0.13, 0.055, -35)
        c.fill(lid.transform(lid.size, Image.AFFINE, (1, 0, -c.px(0.015), 0, 1, -c.px(0.02))), (0, 0, 0, 160))
        c.gradient(lid, rgb("#f6f0e6"), rgb("#7a7268"), angle_down=False)
        c.fill(outline_of(c, lid, 0.011), rgb("#3a332d"))
        if big:
            c.fill(rotated_ellipse_mask(c, lx - 0.01, ly - 0.004, 0.08, 0.025, -35), (255, 255, 255, 90))

    if mid:
        # what was inside, bursting out: past the frame, even
        loot = [("diamond", 0.69, 0.13, 0.075, RED_HOT, RED_DEEP),
                ("star", 0.86, 0.3, 0.075, GOLD, EMBER)]
        if big:
            loot.append(("frame", 0.52, 0.09, 0.055, BONE, BONE_DIM))
        for shape, x, y, r, top_c, bottom_c in loot:
            if shape == "diamond":
                m = polygon_mask(c, [(x, y - r), (x + r * 0.8, y), (x, y + r), (x - r * 0.8, y)])
            elif shape == "star":
                m = star_mask(c, x, y, r, spin=0.3)
            else:
                m = polygon_mask(c, rotated_rect(x, y, r, r * 0.75, 0.35))
            if big:
                # a short trail back towards the can
                c.fill(bar_mask(c, x - (x - ox) * 0.45, y - (y - oy) * 0.45, x, y, r * 0.35), (255, 200, 120, 70))
            c.fill(m.transform(m.size, Image.AFFINE, (1, 0, -c.px(0.012), 0, 1, -c.px(0.016))), (0, 0, 0, 170))
            c.gradient(m, top_c, bottom_c, angle_down=False)
            if size >= 32:
                c.fill(outline_of(c, m, 0.01), SOOT)

    if mid:
        # sparks flying off the torn metal
        sparks = c.mask()
        d = ImageDraw.Draw(sparks)
        for _ in range(9 if big else 5):
            ang = rng.uniform(-np.pi * 0.95, -np.pi * 0.05)
            r0 = rng.uniform(0.04, 0.1)
            r1 = r0 + rng.uniform(0.05, 0.13)
            sx, sy = ox + rng.uniform(-0.08, 0.08), oy + rng.uniform(-0.02, 0.03)
            d.line([c.px(sx + np.cos(ang) * r0), c.px(sy + np.sin(ang) * r0),
                    c.px(sx + np.cos(ang) * r1), c.px(sy + np.sin(ang) * r1)],
                   fill=255, width=max(1, int(c.px(0.012))))
        glow(c, sparks, (255, 150, 40, 200), 0.02)
        c.fill(sparks, SPARK)

    return body


def rotated_rect(cx, cy, w, h, angle):
    pts = []
    for sx, sy in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
        x, y = sx * w, sy * h
        pts.append((cx + x * np.cos(angle) - y * np.sin(angle), cy + x * np.sin(angle) + y * np.cos(angle)))
    return pts


def ellipse_mask(c, cx, cy, rx, ry):
    m = c.mask()
    ImageDraw.Draw(m).ellipse([c.px(cx - rx), c.px(cy - ry), c.px(cx + rx), c.px(cy + ry)], fill=255)
    return m


def rotated_ellipse_mask(c, cx, cy, rx, ry, degrees):
    a = np.radians(degrees)
    pts = []
    for k in range(48):
        t = 2 * np.pi * k / 48
        x, y = np.cos(t) * rx, np.sin(t) * ry
        pts.append((cx + x * np.cos(a) - y * np.sin(a), cy + x * np.sin(a) + y * np.cos(a)))
    return polygon_mask(c, pts)


def metal_array(n, x0, x1):
    """Tin: bright down one side, dark at the edges, like a cylinder."""
    xs = np.linspace(0, 1, n, dtype=np.float32)
    t = np.clip((xs - x0) / (x1 - x0), 0, 1)
    shade = 0.45 + 0.55 * np.clip(np.sin(np.pi * np.clip(t * 1.15 - 0.05, 0, 1)), 0, 1) ** 0.8
    shade += 0.35 * np.exp(-((t - 0.3) / 0.07) ** 2)
    shade = np.clip(shade, 0, 1.25)
    base = np.array([196, 188, 176], np.float32)
    row = np.clip(base[None, :] * shade[:, None], 0, 255)
    out = np.zeros((n, n, 4), np.float32)
    out[..., :3] = row[None, :, :]
    out[..., 3] = 255
    return out


# ------------------------------------------------------------- final pass --

def dirty(pixels, body_mask, rng_np, dirt, size):
    """Dirt at the real pixel size: grain, scanlines and specks."""
    if dirt <= 0:
        return pixels
    inside = (np.asarray(body_mask.resize((size, size), Image.BOX), np.float32) / 255.0)[..., None]
    sigma = {1: 4.0, 2: 6.0, 3: 8.5}[dirt]
    grain = rng_np.normal(0, sigma, (size, size, 1)).astype(np.float32)
    pixels[..., :3] += grain * inside
    if dirt >= 2 and size >= 48:
        rows = (np.arange(size) % 2 == 1).astype(np.float32)[:, None, None]
        pixels[..., :3] *= 1 - 0.07 * rows * inside
    if dirt >= 2:
        specks = (rng_np.random((size, size, 1)) < 0.012).astype(np.float32)
        pixels[..., :3] *= 1 - 0.35 * specks * inside
    return np.clip(pixels, 0, 255)


def label(pixels, text, bg, fg, size):
    """The extension in a pixel font, on a strip along the bottom."""
    scale = max(1, size // 48)
    glyph_h = 5 * scale
    gap = scale
    widths = [len(FONT[ch][0]) * scale for ch in text]
    width = sum(widths) + (len(text) - 1) * gap
    pad = max(1, scale)
    strip_h = glyph_h + pad * 2 + scale
    m = int(round(MARGIN * size)) + max(1, round(size * 0.05))
    y0 = size - m - strip_h - max(1, scale)
    x0 = m + max(1, scale)
    x1 = size - m - max(1, scale)
    pixels[y0:y0 + strip_h, x0:x1, :3] = np.array(bg[:3], np.float32)
    pixels[y0:y0 + strip_h, x0:x1, 3] = 255
    # a darker underline and lighter top edge, like a sticker strip
    pixels[y0 + strip_h - 1, x0:x1, :3] *= 0.6
    pixels[y0, x0:x1, :3] = np.minimum(255, pixels[y0, x0:x1, :3] * 1.18)
    tx = x0 + ((x1 - x0) - width) // 2
    ty = y0 + pad + scale // 2
    for ch, glyph_w in zip(text, widths):
        rows = FONT[ch]
        for gy, row in enumerate(rows):
            for gx, bit in enumerate(row):
                if bit == "1":
                    pixels[ty + gy * scale: ty + (gy + 1) * scale,
                           tx + gx * scale: tx + (gx + 1) * scale, :3] = np.array(fg[:3], np.float32)
        tx += glyph_w + gap
    return pixels


# ----------------------------------------------------------------- icons --

def dirt_for(size):
    return 0 if size <= 16 else 1 if size <= 24 else 2 if size <= 48 else 3


def render(name, size):
    seed = sum(ord(ch) for ch in name) * 1000 + size
    rng = random.Random(seed)
    rng_np = np.random.default_rng(seed)
    dirt = dirt_for(size)
    c = Canvas(size)
    file_kind = name if name in LABELS else None
    fold = 0.24 if file_kind else 0
    body = draw_chip(c, rng, fold, dirt)
    labelled = file_kind is not None and size >= 48

    if name == "canripper":
        draw_can(c, rng, dirt)
    else:
        scale = 0.86 if labelled else (0.94 if file_kind else 1.0)
        cx, cy = 0.5, (0.43 if labelled else 0.49)
        draw_q(c, cx, cy, scale, dirt)
        if file_kind:
            g = glyph(c, file_kind, cx, cy, scale)
            if size >= 32:
                c.fill(g.transform(g.size, Image.AFFINE, (1, 0, -c.px(0.012), 0, 1, -c.px(0.014))), RED_GHOST)
            c.gradient(g, RED_HOT, RED_DEEP, angle_down=False)
            if file_kind == "q1s":
                pencil_tip(c, cx, cy, scale)
        else:
            mark = "square" if name == "q0editor" else "triangle"
            s = 0.15 if size > 16 else 0.2
            sticker(c, mark, MARGIN + 0.07, MARGIN + 0.07, s)

    pixels = c.shrink()
    pixels = dirty(pixels, body, rng_np, dirt, size)
    if labelled:
        bg, fg = LABELS[file_kind]
        pixels = label(pixels, file_kind.upper(), bg, fg, size)
    return Image.fromarray(pixels.astype(np.uint8), "RGBA")


# ------------------------------------------------------------------ files --

def bmp_entry(img):
    size = img.size[0]
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    a = np.asarray(img)
    color = a[::-1, :, [2, 1, 0, 3]].tobytes()
    row_bytes = ((size + 31) // 32) * 4
    mask = bytearray()
    for y in range(size - 1, -1, -1):
        row = bytearray(row_bytes)
        for x in range(size):
            if a[y, x, 3] == 0:
                row[x // 8] |= 1 << (7 - (x % 8))
        mask += row
    return header + color + bytes(mask)


def png_entry(img):
    out = io.BytesIO()
    img.save(out, "PNG", optimize=True)
    return out.getvalue()


def write_ico(path, images):
    """Bitmap entries up to 64 (q0icon and old shells read those), png above."""
    entries = [(img.size[0], png_entry(img) if img.size[0] > 64 else bmp_entry(img)) for img in images]
    header = struct.pack("<HHH", 0, 1, len(entries))
    directory = b""
    offset = 6 + 16 * len(entries)
    for size, data in entries:
        dim = 0 if size >= 256 else size
        directory += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    path.write_bytes(header + directory + b"".join(data for _, data in entries))


ICONS = ["q0editor", "q0player", "canripper", "q0s", "q0v", "q1s", "q0l"]


def main():
    ASSETS.mkdir(exist_ok=True)
    rendered = {}
    for name in ICONS:
        images = [render(name, s) for s in ICO_SIZES]
        rendered[name] = images
        write_ico(ASSETS / f"{name}.ico", images)
        print(f"  {name}.ico")
    for app, name in (("app", "q0editor"), ("player", "q0player")):
        for density, size in ANDROID.items():
            path = ROOT / "android" / app / "src" / "main" / "res" / f"mipmap-{density}" / "ic_launcher.png"
            render(name, size).save(path, optimize=True)
        print(f"  android {app} launcher icons")
    preview(rendered)


def preview(rendered):
    """Every icon at every size, on light and dark, and blown up at 4x."""
    cell = 270
    sheet = Image.new("RGBA", (cell * len(ICONS), 760), (0, 0, 0, 255))
    d = ImageDraw.Draw(sheet)
    d.rectangle([0, 0, sheet.size[0], 380], fill=(214, 210, 204, 255))
    d.rectangle([0, 380, sheet.size[0], 760], fill=(32, 32, 36, 255))
    for i, name in enumerate(ICONS):
        x = i * cell + 8
        for band, top in ((0, 8), (1, 388)):
            sheet.alpha_composite(rendered[name][-1].resize((128, 128), Image.LANCZOS), (x, top))
            xx, yy = x + 136, top
            for img in rendered[name][:6]:
                sheet.alpha_composite(img, (xx, yy))
                yy += img.size[1] + 4
            big = rendered[name][3].resize((128, 128), Image.NEAREST)
            sheet.alpha_composite(big, (x, top + 140))
            small = rendered[name][0].resize((96, 96), Image.NEAREST)
            sheet.alpha_composite(small, (x + 136, top + 236))
            d.text((x, top + 352), name, fill=(0, 0, 0, 255) if band == 0 else (230, 230, 230, 255))
    out = ROOT / "out"
    out.mkdir(exist_ok=True)
    sheet.save(out / "icons-preview.png")
    print("  out/icons-preview.png")


if __name__ == "__main__":
    main()
