#!/usr/bin/env python3
"""Generate Tessera's four original MIT-licensed vector cursor groups.

User light/dark and AI light/dark are complete SVG themes with standard aliases.
User themes use solid geometric silhouettes; AI themes use open-fork directional
tips and negative-space counterforms to visually identify Agent operations.

Run scripts/prepare-tessera-cursors.py [--out DIR]. The output defaults to
assets/cursors; only known generated files are overwritten. Light/dark name
the fill polarity, not the background. Hotspots use a 256-unit viewBox.
"""

from __future__ import annotations

import argparse
import math
import xml.etree.ElementTree as ET
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_OUT = REPO / "assets" / "cursors"

FILL = "#FFFFFF"
INK = "#000000"
OUTLINE = 10  # visible ink band width, in viewBox units
CENTER = 128.0


# ---------------------------------------------------------------------------
# SVG element plumbing
# ---------------------------------------------------------------------------


def n(v: float) -> str:
    """Compact number formatting for path data."""
    s = f"{v:.1f}"
    return s[:-2] if s.endswith(".0") else s


def _d(points: list[tuple[float, float]], close: bool = True) -> str:
    d = "M" + "L".join(f"{n(x)} {n(y)}" for x, y in points)
    return d + ("Z" if close else "")


def poly(
    points: list[tuple[float, float]],
    fill: str = FILL,
    stroke: str = INK,
    sw: float = OUTLINE,
    extra: str = "",
) -> str:
    return (
        f'<path d="{_d(points)}" fill="{fill}" stroke="{stroke}" '
        f'stroke-width="{n(sw)}" stroke-linejoin="round" stroke-linecap="round"{extra}/>'
    )


def circle(cx: float, cy: float, r: float, fill=FILL, stroke=INK, sw=OUTLINE) -> str:
    return (
        f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{fill}" '
        f'stroke="{stroke}" stroke-width="{n(sw)}"/>'
    )


def dot(cx: float, cy: float, r: float) -> str:
    return f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{INK}"/>'


def line(x1: float, y1: float, x2: float, y2: float, sw: float) -> str:
    return (
        f'<path d="M{n(x1)} {n(y1)}L{n(x2)} {n(y2)}" fill="none" stroke="{INK}" '
        f'stroke-width="{n(sw)}" stroke-linecap="round"/>'
    )


def stroke_path(d: str, sw: float) -> str:
    return (
        f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{n(sw)}" '
        f'stroke-linecap="round"/>'
    )


def rotate(
    points: list[tuple[float, float]], deg: float, c: float = CENTER
) -> list[tuple[float, float]]:
    """Rotate points about (c, c); positive angles turn clockwise on screen."""
    rad = math.radians(deg)
    co, si = math.cos(rad), math.sin(rad)
    return [
        (c + (x - c) * co - (y - c) * si, c + (x - c) * si + (y - c) * co)
        for x, y in points
    ]


# ---------------------------------------------------------------------------
# Union-of-primitives builder (two-pass paint; see module docstring)
# ---------------------------------------------------------------------------

# Primitives: ("rect", x, y, w, h, rx) | ("circle", cx, cy, r)
#             ("capsule", x1, y1, x2, y2, width)
Prim = tuple


def _prim_svg(p: Prim, ink: bool, out: float) -> str:
    color = INK if ink else FILL
    grow = 2.0 * out if ink else 0.0
    kind = p[0]
    if kind == "rect":
        _, x, y, w, h, rx = p
        stroke = f'stroke="{color}" stroke-width="{n(grow)}"' if ink else ""
        return (
            f'<rect x="{n(x)}" y="{n(y)}" width="{n(w)}" height="{n(h)}" '
            f'rx="{n(rx)}" fill="{color}" {stroke}/>'
        )
    if kind == "circle":
        _, cx, cy, r = p
        stroke = f'stroke="{color}" stroke-width="{n(grow)}"' if ink else ""
        return f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{color}" {stroke}/>'
    if kind == "capsule":
        _, x1, y1, x2, y2, w = p
        return (
            f'<path d="M{n(x1)} {n(y1)}L{n(x2)} {n(y2)}" fill="none" '
            f'stroke="{color}" stroke-width="{n(w + grow)}" stroke-linecap="round"/>'
        )
    raise ValueError(f"unknown primitive {kind!r}")


def union(prims: list[Prim], out: float = OUTLINE) -> str:
    return "".join(_prim_svg(p, True, out) for p in prims) + "".join(
        _prim_svg(p, False, out) for p in prims
    )


# ---------------------------------------------------------------------------
# Shape geometry
# ---------------------------------------------------------------------------

# User pointer: upright leading edge, compact shoulder and narrow stem.
# Rounded joins soften the silhouette without blunting the pointing tip.
ARROW = [(64, 28), (192, 142), (134, 146), (164, 210), (137, 223), (106, 157), (64, 198)]
ARROW_HOTSPOT = (64, 28)

# Badge position for arrow+badge compound cursors (bottom-right of the arrow).
BADGE = (190.0, 184.0, 38.0)  # cx, cy, r


def ai_pointer() -> str:
    # Open fork: a directional tip with two separated tails. Negative space
    # distinguishes the observer from the user's solid stem without color.
    return outlined_path("M64 28L202 174L140 158L110 222Z") + (
        f'<path d="M83 68L137 154L113 197Z" fill="{INK}"/>'
    )


def ai_right_ptr() -> str:
    return outlined_path("M192 28L54 174L116 158L146 222Z") + (
        f'<path d="M173 68L119 154L143 197Z" fill="{INK}"/>'
    )


def arrow(role: str = "user") -> str:
    return ai_pointer() if role == "ai" else poly(ARROW)


def right_ptr(role: str = "user") -> str:
    return ai_right_ptr() if role == "ai" else poly([(256 - x, y) for x, y in ARROW])


# I-beam: vertical bar with straight serifs.
_XTERM_PTS = [
    (88, 40),
    (168, 40),
    (168, 54),
    (140, 54),
    (140, 202),
    (168, 202),
    (168, 216),
    (88, 216),
    (88, 202),
    (116, 202),
    (116, 54),
    (88, 54),
]


def ai_xterm() -> str:
    return poly(_XTERM_PTS) + f'<path d="M110 128H146" stroke="{INK}" stroke-width="8" stroke-linecap="round"/>'


def xterm(role: str = "user") -> str:
    return ai_xterm() if role == "ai" else poly(_XTERM_PTS)


def vertical_text(role: str = "user") -> str:
    return poly(rotate(_XTERM_PTS, 90))


def _plus(half_arm: float, half_span: float) -> list[tuple[float, float]]:
    a, s, c = half_arm, half_span, CENTER
    return [
        (c - a, c - s),
        (c + a, c - s),
        (c + a, c - a),
        (c + s, c - a),
        (c + s, c + a),
        (c + a, c + a),
        (c + a, c + s),
        (c - a, c + s),
        (c - a, c + a),
        (c - s, c + a),
        (c - s, c - a),
        (c - a, c - a),
    ]


def crosshair() -> str:
    return poly(_plus(12, 80))


def cell() -> str:
    # Chunky plus with a punched-out centre square (the "cell" read).
    hole = "M116 116L140 116L140 140L116 140Z"
    return (
        f'<path d="{_d(_plus(17, 72))}{hole}" fill="{FILL}" stroke="{INK}" '
        f'stroke-width="{n(OUTLINE)}" stroke-linejoin="round" fill-rule="evenodd"/>'
    )


def ring(cx: float, cy: float, radius: float, width: float) -> str:
    # A filled annulus keeps an opposing outline on BOTH edges; a lone
    # monochrome stroke disappears over content of the same luminance.
    def loop(r: float) -> str:
        return (f"M{cx-r} {cy}a{r} {r} 0 1 0 {2*r} 0"
                f"a{r} {r} 0 1 0 {-2*r} 0Z")
    return (f'<path d="{loop(radius)}{loop(radius-width)}" fill="{FILL}" '
            f'stroke="{INK}" stroke-width="8" fill-rule="evenodd"/>')


def watch() -> str:
    return ring(128, 128, 70, 20) + poly([(128, 44), (158, 65), (128, 85)], sw=8)


def _badge(symbol: str) -> str:
    cx, cy, r = BADGE
    return circle(cx, cy, r, sw=12) + symbol


def _badge_q() -> str:
    cx, cy, _ = BADGE
    arc = f"M{n(cx - 13)} {n(cy - 6)}A15 15 0 1 1 {n(cx + 1)} {n(cy + 8)}L{n(cx + 1)} {n(cy + 14)}"
    return _badge(stroke_path(arc, 11) + dot(cx + 1, cy + 24, 6.5))


def _badge_plus() -> str:
    cx, cy, _ = BADGE
    return _badge(line(cx - 15, cy, cx + 15, cy, 11) + line(cx, cy - 15, cx, cy + 15, 11))


def _badge_menu() -> str:
    cx, cy, _ = BADGE
    return _badge(
        line(cx - 14, cy - 12, cx + 14, cy - 12, 9)
        + line(cx - 14, cy, cx + 14, cy, 9)
        + line(cx - 14, cy + 12, cx + 14, cy + 12, 9)
    )


def _badge_shortcut() -> str:
    cx, cy, _ = BADGE
    return _badge(
        stroke_path(
            f"M{n(cx - 13)} {n(cy + 13)}L{n(cx + 13)} {n(cy - 13)}"
            f"L{n(cx + 1)} {n(cy - 13)}M{n(cx + 13)} {n(cy - 13)}L{n(cx + 13)} {n(cy - 1)}",
            10,
        )
    )


def _badge_slash() -> str:
    cx, cy, _ = BADGE
    return _badge(line(cx - 17, cy + 17, cx + 17, cy - 17, 11))


def question_arrow(role: str = "user") -> str:
    return arrow(role) + _badge_q()


def context_menu(role: str = "user") -> str:
    return arrow(role) + _badge_menu()


def alias_cursor(role: str = "user") -> str:
    return arrow(role) + _badge_shortcut()


def copy_cursor(role: str = "user") -> str:
    return arrow(role) + _badge_plus()


def no_drop(role: str = "user") -> str:
    return arrow(role) + _badge_slash()


def left_ptr_watch(role: str = "user") -> str:
    cx, cy, _ = BADGE
    return arrow(role) + ring(cx, cy, 32, 14) + dot(cx, cy - 32, 9)


def not_allowed() -> str:
    return circle(CENTER, CENTER, 74) + line(82, 174, 174, 82, 14)


def zoom(plus: bool) -> str:
    lens = [("circle", 108, 108, 54), ("capsule", 150, 150, 206, 206, 22)]
    mark = line(88, 108, 128, 108, 12)
    if plus:
        mark = line(108, 88, 108, 128, 12) + mark
    return union(lens) + mark


def zoom_in() -> str:
    return zoom(True)


def zoom_out() -> str:
    return zoom(False)


def fleur() -> str:
    c, r_tip, r_base, half_head, half_arm = CENTER, 92, 52, 26, 13
    pts = [
        (c, c - r_tip),
        (c + half_head, c - r_base),
        (c + half_arm, c - r_base),
        (c + half_arm, c - half_arm),
        (c + r_base, c - half_arm),
        (c + r_base, c - half_head),
        (c + r_tip, c),
        (c + r_base, c + half_head),
        (c + r_base, c + half_arm),
        (c + half_arm, c + half_arm),
        (c + half_arm, c + r_base),
        (c + half_head, c + r_base),
        (c, c + r_tip),
        (c - half_head, c + r_base),
        (c - half_arm, c + r_base),
        (c - half_arm, c + half_arm),
        (c - r_base, c + half_arm),
        (c - r_base, c + half_head),
        (c - r_tip, c),
        (c - r_base, c - half_head),
        (c - r_base, c - half_arm),
        (c - half_arm, c - half_arm),
        (c - half_arm, c - r_base),
        (c - half_head, c - r_base),
    ]
    return poly(pts)


# East-pointing single-headed arrow with a flat tail end; rotated for the
# other seven directions.
_SINGLE_E = [(216, 128), (172, 98), (172, 115), (48, 115), (48, 141), (172, 141), (172, 158)]

# East-west double-headed arrow; rotated for the other axes.
_DOUBLE_EW = [
    (40, 128),
    (84, 98),
    (84, 115),
    (172, 115),
    (172, 98),
    (216, 128),
    (172, 158),
    (172, 141),
    (84, 141),
    (84, 158),
]


def single(deg: float) -> str:
    return poly(rotate(_SINGLE_E, deg))


def double(deg: float) -> str:
    return poly(rotate(_DOUBLE_EW, deg))


def col_resize() -> str:
    # Horizontal double arrow with a raised vertical divider bar on top.
    bar = '<rect x="114" y="82" width="28" height="92" rx="8" fill="#FFFFFF" stroke="#000000" stroke-width="12"/>'
    return poly(_DOUBLE_EW) + bar


def row_resize() -> str:
    bar = '<rect x="82" y="114" width="92" height="28" rx="8" fill="#FFFFFF" stroke="#000000" stroke-width="12"/>'
    return poly(rotate(_DOUBLE_EW, 90)) + bar


# --- Hands (unions of capsules / rounded rects / circles) --------------------


def hand2() -> str:
    return outlined_path("M104 122V57Q104 43 118 43Q132 43 132 57V104"
                         "Q148 91 160 107Q179 100 187 119Q206 118 206 139"
                         "V161Q206 188 181 211H116L69 150Q60 136 71 127"
                         "Q82 119 95 135L104 146Z")


def hand1() -> str:
    return outlined_path("M85 130V79Q85 62 99 62Q112 62 112 79V117"
                         "V62Q112 46 126 46Q140 46 140 62V116"
                         "V69Q140 53 154 53Q168 53 168 69V121"
                         "V85Q168 71 181 71Q195 71 195 85V158"
                         "Q195 187 173 211H112L61 150Q50 136 61 126"
                         "Q71 116 85 130Z")


def closedhand() -> str:
    return outlined_path("M77 133V107Q77 91 93 91Q105 91 110 102"
                         "Q113 82 129 84Q143 84 145 99Q153 85 166 91"
                         "Q177 95 178 108Q194 99 202 114V164"
                         "Q202 190 177 208H109Q87 190 77 170"
                         "L61 146Q55 133 66 126Q72 123 77 133Z")


def ai_hand2() -> str:
    return hand2() + f'<path d="M118 70V100" stroke="{INK}" stroke-width="8" stroke-linecap="round"/>'


def ai_hand1() -> str:
    return hand1() + f'<path d="M126 75V105" stroke="{INK}" stroke-width="8" stroke-linecap="round"/>'


def ai_closedhand() -> str:
    return closedhand() + f'<path d="M129 110V135" stroke="{INK}" stroke-width="8" stroke-linecap="round"/>'


def hand2_cursor(role: str = "user") -> str:
    return ai_hand2() if role == "ai" else hand2()


def hand1_cursor(role: str = "user") -> str:
    return ai_hand1() if role == "ai" else hand1()


def closedhand_cursor(role: str = "user") -> str:
    return ai_closedhand() if role == "ai" else closedhand()


def outlined_path(d: str) -> str:
    return (f'<path d="{d}" fill="{FILL}" stroke="{INK}" '
            f'stroke-width="{OUTLINE}" stroke-linejoin="round"/>')


# ---------------------------------------------------------------------------
# Theme table: canonical name -> (builder, hotspot, aliases)
# ---------------------------------------------------------------------------

THEME: list[tuple[str, object, tuple[float, float], list[str]]] = [
    ("left_ptr", arrow, ARROW_HOTSPOT, ["default", "arrow", "top_left_arrow"]),
    ("right_ptr", right_ptr, (256 - ARROW_HOTSPOT[0], ARROW_HOTSPOT[1]), []),
    ("xterm", xterm, (128, 128), ["text", "ibeam"]),
    ("vertical-text", vertical_text, (128, 128), []),
    ("crosshair", crosshair, (128, 128), ["cross", "plus", "tcross"]),
    ("cell", cell, (128, 128), []),
    ("watch", watch, (128, 128), ["wait"]),
    ("left_ptr_watch", left_ptr_watch, ARROW_HOTSPOT, ["progress"]),
    ("question_arrow", question_arrow, ARROW_HOTSPOT, ["help", "left_ptr_help", "whats_this", "dnd-ask"]),
    ("context-menu", context_menu, ARROW_HOTSPOT, []),
    ("alias", alias_cursor, ARROW_HOTSPOT, ["link", "dnd-link"]),
    ("copy", copy_cursor, ARROW_HOTSPOT, ["dnd-copy"]),
    ("no-drop", no_drop, ARROW_HOTSPOT, ["dnd-no-drop", "dnd_no_drop"]),
    ("not-allowed", not_allowed, (128, 128), ["forbidden", "crossed_circle", "circle", "dnd-none"]),
    ("zoom-in", zoom_in, (108, 108), ["zoom_in"]),
    ("zoom-out", zoom_out, (108, 108), ["zoom_out"]),
    ("fleur", fleur, (128, 128), ["move", "all-scroll", "all-resize", "size_all", "dnd-move"]),
    ("hand2", hand2_cursor, (118, 46), ["pointer", "pointing_hand", "hand"]),
    ("hand1", hand1_cursor, (128, 128), ["grab", "openhand"]),
    ("closedhand", closedhand_cursor, (128, 128), ["grabbing"]),
    ("right_side", lambda: single(0), (128, 128), ["e-resize", "sb_right_arrow", "right-arrow"]),
    ("bottom_right_corner", lambda: single(45), (128, 128), ["se-resize", "lr_angle"]),
    ("bottom_side", lambda: single(90), (128, 128), ["s-resize", "sb_down_arrow", "down-arrow"]),
    ("bottom_left_corner", lambda: single(135), (128, 128), ["sw-resize", "ll_angle"]),
    ("left_side", lambda: single(180), (128, 128), ["w-resize", "sb_left_arrow", "left-arrow"]),
    ("top_left_corner", lambda: single(225), (128, 128), ["nw-resize", "ul_angle"]),
    ("top_side", lambda: single(270), (128, 128), ["n-resize", "sb_up_arrow", "up-arrow"]),
    ("top_right_corner", lambda: single(315), (128, 128), ["ne-resize", "ur_angle"]),
    ("sb_h_double_arrow", lambda: double(0), (128, 128), ["ew-resize", "h_double_arrow", "size_hor", "size-hor", "double_arrow"]),
    ("sb_v_double_arrow", lambda: double(90), (128, 128), ["ns-resize", "v_double_arrow", "size_ver", "size-ver"]),
    ("fd_double_arrow", lambda: double(45), (128, 128), ["nwse-resize", "size_fdiag"]),
    ("bd_double_arrow", lambda: double(135), (128, 128), ["nesw-resize", "size_bdiag"]),
    ("col-resize", col_resize, (128, 128), ["split_h"]),
    ("row-resize", row_resize, (128, 128), ["split_v"]),
]

SVG_HEAD = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" '
    'viewBox="0 0 256 256" data-hotspot-x="{hx}" data-hotspot-y="{hy}">'
)


def render_svg(content: str, hotspot: tuple[float, float]) -> str:
    return SVG_HEAD.format(hx=n(hotspot[0]), hy=n(hotspot[1])) + "\n" + content + "\n</svg>\n"


PALETTES = {
    "light": ("#F5F7FA", "#202630"),
    "dark": ("#202630", "#F5F7FA"),
}


def palette(svg: str, polarity: str) -> str:
    fill, ink = PALETTES[polarity]
    return svg.replace(FILL, fill).replace(INK, ink)


def write_preview(root: Path, destination: Path) -> None:
    """Contact sheet from the actual assets, at enlarged and native sizes."""
    parts = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="1120" height="700" viewBox="0 0 1120 700">',
        '<rect width="1120" height="700" fill="#e7e9ee"/>',
        '<text x="32" y="42" font-family="sans-serif" font-size="24" fill="#202630">Tessera / cursor families</text>',
    ]
    groups = [(role, tone) for role in ("user", "ai") for tone in PALETTES]
    names = ["default", "pointer", "text", "grab", "grabbing", "ew-resize", "nwse-resize", "wait", "copy", "not-allowed"]
    for row, (role, tone) in enumerate(groups):
        y = 65 + row * 152
        parts.append(f'<text x="32" y="{y+22}" font-family="sans-serif" font-size="15" fill="#202630">tessera-{role}-{tone}</text>')
        for panel, background in enumerate(["#ffffff", "#202630"]):
            x = 32 + panel * 540
            parts.append(f'<rect x="{x}" y="{y+34}" width="524" height="98" rx="12" fill="{background}"/>')
            for column, name in enumerate(names):
                folder = root / f"tessera-{role}-{tone}" / "cursors"
                svg = ET.fromstring((folder / f"{name}.svg").read_text())
                for size, offset in [(40, 45), (24, 96)]:
                    svg.attrib.update(x=str(x+12+column*50), y=str(y+offset), width=str(size), height=str(size))
                    parts.append(ET.tostring(svg, encoding="unicode"))
    parts.append("</svg>\n")
    destination.write_text("".join(parts))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output groups root")
    ap.add_argument("--preview", type=Path, help="write an SVG contact sheet")
    args = ap.parse_args()
    for polarity in PALETTES:
        for role in ("user", "ai"):
            name = f"tessera-{role}-{polarity}"
            out = args.out / name
            cursors = out / "cursors"
            cursors.mkdir(parents=True, exist_ok=True)
            for canonical, builder, hotspot, aliases in THEME:
                try:
                    content = builder(role=role)
                except TypeError:
                    content = builder()
                svg = palette(render_svg(content, hotspot), polarity)
                for key in [canonical, *aliases]:
                    (cursors / f"{key}.svg").write_text(svg)
            inherits = f"Inherits=tessera-user-{polarity}\n" if role == "ai" else ""
            (out / "index.theme").write_text(
                f"[Icon Theme]\nName=Tessera {'AI' if role == 'ai' else 'User'} {polarity.title()}\n"
                f"Comment=Original MIT-licensed {role} cursor art.\n"
                f"Directories=cursors\n{inherits}\n"
                f"[cursors]\nSize=24\nMinSize=8\nMaxSize=512\nType=Fixed\n"
            )
            print(f"wrote {name}")
    if args.preview:
        write_preview(args.out, args.preview)


if __name__ == "__main__":
    main()
