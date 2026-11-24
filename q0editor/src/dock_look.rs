//! How the dock looks and moves: its tab buttons, the tab being carried,
//! and the places a carried tab can be dropped.
//!
//! egui_dock (vendored, see `vendor/egui_dock/VENDORED.md`) lays all of it
//! out and decides where a tab lands; this draws it, animated. The open tab
//! is a card that slides from tab to tab, its front edge leading and its
//! back edge catching up. A picked-up tab lifts off its bar with a shadow,
//! trails the pointer and leans into the way it moves. The drop buttons pop
//! in one after another, and the place the tab would land glides from target
//! to target, then flashes as the tab lands.
//!
//! Nothing here loops on its own: every motion is a glide, pop or fade
//! started by the pointer, and asks for repaints only until it settles.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use egui::epaint::{tessellator::path::rounded_rectangle, TextShape};
use egui::{
    pos2, vec2, Color32, Context, FontId, Id, LayerId, Order, Painter, Pos2, Rect, Rounding, Shape,
    Stroke, TextStyle, Ui, Vec2,
};
use egui_dock::q0s::{DropButton, DropKind, Overlay, TabLook};
use egui_dock::Split;

use crate::ui::Icon;

/// Room in front of a tab's title for its icon.
pub(crate) const ICON_ROOM: f32 = 19.0;
const ICON_SIZE: f32 = 13.0;
/// Space between a tab's edge and its icon.
const PAD: f32 = 8.0;
/// How far below the bar's top the open tab's card starts.
const CARD_TOP: f32 = 3.0;
const CLOSE_RED: Color32 = Color32::from_rgb(0xD9, 0x4A, 0x4A);

/// The colours the dock is drawn in, from the theme.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub accent: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    /// The open tab: the same as the top of its panel, so the two read as
    /// one piece.
    pub card: Color32,
    /// Its rim, and the line along the bottom of a bar.
    pub line: Color32,
    /// Behind a tab the pointer is on.
    pub hover: Color32,
    /// The drop buttons' tiles.
    pub tile: Color32,
}

/// One tab button, noted while the dock lays it out and drawn once its
/// whole bar is known.
pub(crate) struct Tab {
    /// The same for this tab whichever bar it moves to.
    pub key: Id,
    pub icon: Icon,
    pub title: &'static str,
    leaf: Id,
    rect: Rect,
    bar: Rect,
    layer: LayerId,
    clip: Rect,
    hovered: bool,
    pressed: bool,
    close: Option<Close>,
    active: bool,
    focused: bool,
    carried: Option<Pos2>,
}

#[derive(Clone, Copy)]
struct Close {
    rect: Rect,
    hovered: bool,
    pressed: bool,
}

impl Tab {
    pub(crate) fn note(
        ui: &Ui,
        look: &TabLook<'_>,
        key: Id,
        icon: Icon,
        title: &'static str,
    ) -> Self {
        Self {
            key,
            icon,
            title,
            leaf: look.leaf,
            rect: look.rect,
            bar: look.bar,
            layer: ui.layer_id(),
            clip: ui.clip_rect(),
            hovered: look.response.hovered(),
            pressed: look.response.is_pointer_button_down_on(),
            close: look.close.map(|close| Close {
                rect: close.rect,
                hovered: close.hovered(),
                pressed: close.is_pointer_button_down_on(),
            }),
            active: look.active,
            focused: look.focused,
            carried: look.carried,
        }
    }
}

/// What the dock showed this frame.
pub(crate) struct Shown {
    pub palette: Palette,
    pub tabs: Vec<Tab>,
    /// The drop overlay, while a tab is carried.
    pub overlay: Option<Overlay>,
    /// The workspace edge a carried tab would dock along, if it is that
    /// near one: the area it would take.
    pub edge: Option<Rect>,
    /// The whole docking area.
    pub bounds: Rect,
}

/// Draws the dock's tabs, the carried tab and the drop targets, once the
/// dock has laid them out this frame.
pub(crate) fn paint(ctx: &Context, shown: Shown) {
    let now = ctx.input(|input| input.time);
    let font = TextStyle::Button.resolve(&ctx.style());
    let carried = shown.tabs.iter().find(|tab| tab.carried.is_some());
    note_landing(ctx, carried.map(|tab| tab.key), now);

    let mut leaves: Vec<Id> = Vec::new();
    for tab in &shown.tabs {
        if !leaves.contains(&tab.leaf) {
            leaves.push(tab.leaf);
        }
    }
    for leaf in leaves {
        let tabs: Vec<&Tab> = shown.tabs.iter().filter(|tab| tab.leaf == leaf).collect();
        paint_bar(ctx, &shown.palette, &font, &tabs, now);
    }

    let target = shown.edge.map(|rect| (rect, DropKind::Dock)).or_else(|| {
        shown
            .overlay
            .as_ref()
            .and_then(|overlay| overlay.target)
            .map(|target| (target.rect, target.kind))
    });
    paint_overlay(
        ctx,
        &shown.palette,
        shown.overlay.as_ref().filter(|_| shown.edge.is_none()),
        target,
        carried.is_some().then_some(shown.bounds),
        now,
    );
    if let Some(tab) = carried {
        paint_ghost(
            ctx,
            &shown.palette,
            &font,
            tab,
            target.map(|(_, kind)| kind),
        );
    } else {
        ghost_gone(ctx);
    }
}

// --------------------------------------------------------------- motion

#[derive(Clone, Copy)]
struct Spring {
    value: f32,
    velocity: f32,
    time: f64,
}

/// Follows `target` like a weight on a critically damped spring: quick, no
/// overshoot, and a target that moves mid-glide bends the path instead of
/// restarting it. `period` is about how long it takes to settle. Gives the
/// value and how fast it is moving, in points a second.
fn glide(ctx: &Context, id: Id, target: f32, period: f32) -> (f32, f32) {
    let now = ctx.input(|input| input.time);
    let fresh = Spring {
        value: target,
        velocity: 0.0,
        time: now,
    };
    let mut spring = ctx
        .data(|data| data.get_temp::<Spring>(id))
        .unwrap_or(fresh);
    if period <= 0.0 || !spring.value.is_finite() {
        spring = fresh;
    } else if now > spring.time {
        let dt = ((now - spring.time) as f32).min(0.1);
        let omega = 6.0 / period;
        let offset = spring.value - target;
        let push = spring.velocity + omega * offset;
        let decay = (-omega * dt).exp();
        let offset = (offset + push * dt) * decay;
        spring.velocity = (spring.velocity - omega * push * dt) * decay;
        spring.value = target + offset;
        spring.time = now;
        if offset.abs() < 0.01 && spring.velocity.abs() < 0.5 {
            spring = fresh;
        }
    }
    if spring.value != target {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, spring));
    (spring.value, spring.velocity)
}

/// Where a glide is now, without moving it on.
fn peek(ctx: &Context, id: Id) -> Option<f32> {
    ctx.data(|data| data.get_temp::<Spring>(id))
        .map(|spring| spring.value)
}

/// Starts a glide from `value`, standing still.
fn place(ctx: &Context, id: Id, value: f32) {
    let now = ctx.input(|input| input.time);
    ctx.data_mut(|data| {
        data.insert_temp(
            id,
            Spring {
                value,
                velocity: 0.0,
                time: now,
            },
        )
    });
}

fn glide_rect(ctx: &Context, id: Id, target: Rect, period: f32) -> Rect {
    Rect::from_min_max(
        pos2(
            glide(ctx, id.with(0), target.min.x, period).0,
            glide(ctx, id.with(1), target.min.y, period).0,
        ),
        pos2(
            glide(ctx, id.with(2), target.max.x, period).0,
            glide(ctx, id.with(3), target.max.y, period).0,
        ),
    )
}

fn place_rect(ctx: &Context, id: Id, rect: Rect) {
    for (index, value) in [rect.min.x, rect.min.y, rect.max.x, rect.max.y]
        .into_iter()
        .enumerate()
    {
        place(ctx, id.with(index), value);
    }
}

/// Seconds since `on` last turned true for `id`, while it stays true. Not
/// being asked for a couple of frames counts as having been off, so
/// something that comes back into sight starts over.
fn since_on(ctx: &Context, id: Id, on: bool) -> Option<f32> {
    let now = ctx.input(|input| input.time);
    let frame = ctx.frame_nr();
    let seen: Option<(bool, f64, u64)> = ctx.data(|data| data.get_temp(id));
    let (was, since) = match seen {
        Some((was, since, last)) if last + 2 >= frame => (was, since),
        _ => (false, now),
    };
    let since = if on && !was { now } else { since };
    ctx.data_mut(|data| data.insert_temp(id, (on, since, frame)));
    on.then_some((now - since) as f32)
}

fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Out past 1 and back: what pops in overshoots a little before it settles.
fn ease_back(t: f32) -> f32 {
    const C1: f32 = 1.70158;
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + (C1 + 1.0) * u * u * u + C1 * u * u
}

/// 0 to 1 over `secs` from `start`, asking for repaints until it gets there.
fn progress(ctx: &Context, start: f32, secs: f32) -> f32 {
    let t = (start / secs).clamp(0.0, 1.0);
    if t < 1.0 {
        ctx.request_repaint();
    }
    t
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    Color32::from(egui::lerp(
        egui::Rgba::from(a)..=egui::Rgba::from(b),
        t.clamp(0.0, 1.0),
    ))
}

// ---------------------------------------------------------------- tabs

const CARRIED_ID: &str = "q0editor.dock.carried";

/// When a carried tab is let go, it pops where it lands.
fn note_landing(ctx: &Context, carried: Option<Id>, now: f64) {
    let id = Id::new(CARRIED_ID);
    let was: Option<Id> = ctx.data(|data| data.get_temp(id));
    if let (Some(key), None) = (was, carried) {
        ctx.data_mut(|data| data.insert_temp(key.with("q0s_landed"), now));
    }
    ctx.data_mut(|data| match carried {
        Some(key) => data.insert_temp(id, key),
        None => data.remove::<Id>(id),
    });
}

/// One tab bar: the open tab's card, the line along the bottom, and each
/// tab's icon, title and close button.
fn paint_bar(ctx: &Context, palette: &Palette, font: &FontId, tabs: &[&Tab], now: f64) {
    let Some(first) = tabs.first() else {
        return;
    };
    let bar = first.bar;
    let painter = ctx.layer_painter(first.layer).with_clip_rect(first.clip);
    let active = tabs.iter().find(|tab| tab.active);
    // The card slides from tab to tab: the edge in front leads, the one
    // behind catches up, so it stretches as it goes. A bar that moved or
    // changed size (a panel being resized, a new layout) snaps it instead.
    let span = active.map(|tab| {
        let id = tab.leaf.with((
            "q0editor.dock.card",
            bar.min.x.round() as i32,
            bar.min.y.round() as i32,
            bar.width().round() as i32,
        ));
        let (lead, lag) = (0.1, 0.22);
        let (left_id, right_id) = (id.with("left"), id.with("right"));
        let left_now = peek(ctx, left_id).unwrap_or(tab.rect.left());
        let right_now = peek(ctx, right_id).unwrap_or(tab.rect.right());
        let left = glide(
            ctx,
            left_id,
            tab.rect.left(),
            if tab.rect.left() < left_now {
                lead
            } else {
                lag
            },
        )
        .0;
        let right = glide(
            ctx,
            right_id,
            tab.rect.right(),
            if tab.rect.right() > right_now {
                lead
            } else {
                lag
            },
        )
        .0;
        (left, right.max(left + 4.0))
    });

    let y = bar.bottom() - 0.5;
    let line = Stroke::new(1.0_f32, palette.line);
    match span {
        Some((left, right)) => {
            painter.line_segment([pos2(bar.left(), y), pos2(left, y)], line);
            painter.line_segment([pos2(right, y), pos2(bar.right(), y)], line);
        }
        None => {
            painter.line_segment([pos2(bar.left(), y), pos2(bar.right(), y)], line);
        }
    }
    if let (Some(tab), Some((left, right))) = (active, span) {
        let focus = ctx.animate_bool_with_time(tab.leaf.with("q0s_focus"), tab.focused, 0.2);
        let card = Rect::from_min_max(pos2(left, bar.top() + CARD_TOP), pos2(right, bar.bottom()));
        paint_card(ctx, &painter, palette, card, focus);
    }
    for tab in tabs {
        paint_tab(ctx, &painter, palette, font, tab, now);
    }
}

/// The open tab's card: rounded at the top, open at the bottom into its
/// panel, with the accent along its top, brighter in the panel last used.
fn paint_card(ctx: &Context, painter: &Painter, palette: &Palette, card: Rect, focus: f32) {
    let top = |radius: f32| Rounding {
        nw: radius,
        ne: radius,
        sw: 0.0,
        se: 0.0,
    };
    painter.rect_filled(card, top(6.0), palette.line);
    painter.rect_filled(
        Rect::from_min_max(card.min + vec2(1.0, 1.0), card.max + vec2(-1.0, 1.0)),
        top(5.0),
        palette.card,
    );
    let glow = Rect::from_min_max(
        card.min + vec2(1.0, 2.0),
        pos2(card.right() - 1.0, card.top() + 11.0),
    );
    painter.add(crate::ui::gradient_shape(
        ctx,
        glow,
        0.0,
        [
            palette.accent.gamma_multiply(0.05 + 0.13 * focus),
            Color32::TRANSPARENT,
        ],
    ));
    painter.rect_filled(
        Rect::from_min_max(
            card.min + vec2(1.0, 0.0),
            pos2(card.right() - 1.0, card.top() + 2.0),
        ),
        top(5.0),
        palette.accent.gamma_multiply(0.4 + 0.6 * focus),
    );
}

fn paint_tab(
    ctx: &Context,
    painter: &Painter,
    palette: &Palette,
    font: &FontId,
    tab: &Tab,
    now: f64,
) {
    let key = tab.key;
    let near_close = tab.close.is_some_and(|close| close.hovered);
    let lit = (tab.hovered || near_close) && tab.carried.is_none();
    let hover = ctx.animate_bool_with_time(key.with("q0s_hover"), lit, 0.12);
    let open = ctx.animate_bool_with_time(key.with("q0s_open"), tab.active, 0.16);
    let press = ctx.animate_bool_with_time(key.with("q0s_press"), tab.pressed, 0.06);
    let middle_y = (tab.bar.top() + CARD_TOP + tab.bar.bottom()) * 0.5;

    if tab.carried.is_some() {
        // Its place in the bar, kept open while it is carried.
        let slot = Rect::from_min_max(
            pos2(tab.rect.left() + 3.0, tab.bar.top() + CARD_TOP + 2.0),
            pos2(tab.rect.right() - 3.0, tab.bar.bottom() - 2.0),
        );
        dashed_outline(
            painter,
            slot,
            5.0,
            Stroke::new(1.2_f32, palette.accent.gamma_multiply(0.65)),
            (now * 18.0) as f32,
        );
        ctx.request_repaint();
        return;
    }

    // A pill behind a tab the pointer is on, unless it is the open one.
    let pill = hover * (1.0 - open);
    if pill > 0.01 {
        let rect = Rect::from_min_max(
            pos2(tab.rect.left() + 2.0, tab.bar.top() + CARD_TOP + 1.0),
            pos2(tab.rect.right() - 2.0, tab.bar.bottom() - 2.0),
        );
        painter.rect_filled(rect, 5.0, palette.hover.gamma_multiply(pill));
    }

    // It pops in when it first shows, when it opens, and where it lands
    // after being carried.
    let appear =
        since_on(ctx, key.with("q0s_seen"), true).map_or(1.0, |since| progress(ctx, since, 0.3));
    let opened = since_on(ctx, key.with("q0s_opened"), tab.active)
        .map_or(1.0, |since| progress(ctx, since, 0.4));
    let landed = ctx
        .data(|data| data.get_temp::<f64>(key.with("q0s_landed")))
        .map_or(1.0, |at| progress(ctx, (now - at) as f32, 0.45));
    let pop = (0.6 + 0.4 * ease_back(appear))
        * (0.72 + 0.28 * ease_back(opened))
        * (0.78 + 0.22 * ease_back(landed))
        * (1.0 - 0.12 * press);
    let shown = ease_out(appear);

    let text = mix(palette.text_dim, palette.text, open.max(hover * 0.8)).gamma_multiply(shown);
    let icon = mix(
        mix(palette.text_dim, palette.text, hover * 0.8),
        palette.accent,
        open,
    )
    .gamma_multiply(shown);
    // Held and pulled, it strains toward the pointer before it comes away
    // from the bar, and springs back if let go.
    let pull = if tab.pressed {
        ctx.input(|input| {
            input
                .pointer
                .press_origin()
                .zip(input.pointer.latest_pos())
                .map(|(from, to)| to - from)
        })
        .unwrap_or(Vec2::ZERO)
    } else {
        Vec2::ZERO
    };
    let tug = vec2(
        glide(
            ctx,
            key.with("q0s_tug_x"),
            (pull.x * 0.15).clamp(-4.5, 4.5),
            0.12,
        )
        .0,
        glide(
            ctx,
            key.with("q0s_tug_y"),
            (pull.y * 0.3).clamp(-2.0, 2.0),
            0.12,
        )
        .0,
    );
    let y = middle_y + press * 0.8 - hover * (1.0 - open) * 0.7 + tug.y;
    let left = tab.rect.left() + PAD + tug.x;
    crate::ui::paint_icon(
        painter,
        Rect::from_center_size(
            pos2(left + ICON_SIZE * 0.5, y),
            Vec2::splat(ICON_SIZE * pop),
        ),
        tab.icon,
        icon,
    );
    let galley = painter.layout_no_wrap(tab.title.to_owned(), font.clone(), text);
    let at = pos2(left + ICON_ROOM, y - galley.size().y * 0.5 + 0.5);
    painter.galley(at, galley, text);

    if let Some(close) = tab.close {
        paint_close(
            ctx,
            painter,
            palette,
            key,
            close,
            pos2(close.rect.center().x, y),
            lit,
            tab.active,
            shown,
        );
    }
}

/// A tab's close button: an × that turns in as it appears, on the open tab
/// and any tab the pointer is on, and sits on a red dot while it is aimed at.
#[allow(clippy::too_many_arguments)]
fn paint_close(
    ctx: &Context,
    painter: &Painter,
    palette: &Palette,
    key: Id,
    close: Close,
    center: Pos2,
    lit: bool,
    active: bool,
    shown: f32,
) {
    let wanted = if close.hovered || lit {
        1.0
    } else if active {
        0.45
    } else {
        0.0
    };
    let seen = ctx.animate_value_with_time(key.with("q0s_close"), wanted, 0.14);
    if seen <= 0.01 {
        return;
    }
    let hot = ctx.animate_bool_with_time(key.with("q0s_close_hot"), close.hovered, 0.1);
    if hot > 0.01 {
        let radius = (7.5 - if close.pressed { 1.2 } else { 0.0 }) * (0.55 + 0.45 * hot);
        painter.circle_filled(center, radius, CLOSE_RED.gamma_multiply(0.9 * hot));
    }
    let size = 3.4 * (0.55 + 0.45 * seen.min(1.0));
    let turn = (1.0 - seen.min(1.0)) * FRAC_PI_2;
    let color = mix(palette.text_dim, Color32::WHITE, hot).gamma_multiply(seen.min(1.0) * shown);
    for angle in [FRAC_PI_4, 3.0 * FRAC_PI_4] {
        let arm = Vec2::angled(angle + turn) * size;
        painter.line_segment([center - arm, center + arm], Stroke::new(1.4_f32, color));
    }
}

// ----------------------------------------------------------- carried tab

const GHOST_ID: &str = "q0editor.dock.ghost";

/// The carried tab: lifted off its bar with a shadow under it, trailing
/// the pointer a little and leaning into the way it moves. Over a place it
/// would dock in, it shrinks back so the place shows through.
fn paint_ghost(ctx: &Context, palette: &Palette, font: &FontId, tab: &Tab, over: Option<DropKind>) {
    let (Some(grab), Some(pointer)) = (tab.carried, ctx.input(|input| input.pointer.latest_pos()))
    else {
        return;
    };
    let id = Id::new(GHOST_ID);
    let since = since_on(ctx, id.with("lift"), true).unwrap_or(0.0);
    if since <= 0.0 {
        // Picked up just now: it leaves from its place in the bar.
        place(ctx, id.with("x"), tab.rect.min.x);
        place(ctx, id.with("y"), tab.rect.min.y);
        place(ctx, id.with("lean"), 0.0);
    }
    let lift = ease_back(progress(ctx, since, 0.28));
    let aim = pointer - (grab - tab.rect.min);
    let (x, speed) = glide(ctx, id.with("x"), aim.x, 0.08);
    let (y, _) = glide(ctx, id.with("y"), aim.y, 0.08);
    let lean = glide(
        ctx,
        id.with("lean"),
        (speed * 0.0004).clamp(-0.16, 0.16),
        0.14,
    )
    .0;
    let docking = ctx.animate_bool_with_time(
        id.with("docking"),
        matches!(over, Some(DropKind::Dock | DropKind::Tab)),
        0.14,
    );
    let scale = (1.0 + 0.08 * lift) * (1.0 - 0.12 * docking);
    let fade = 1.0 - 0.2 * docking;
    let size = tab.rect.size() * scale;
    let center = pos2(x, y) + tab.rect.size() * 0.5;
    let card = Rect::from_center_size(center, size);
    let (sin, cos) = lean.sin_cos();
    let turn = |point: Pos2| {
        let v = point - center;
        center + vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
    };
    let outline = |rect: Rect, radius: f32| -> Vec<Pos2> {
        let mut points = Vec::new();
        rounded_rectangle(&mut points, rect, Rounding::same(radius));
        points.into_iter().map(turn).collect()
    };

    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, id));
    for (grow, alpha) in [(7.0, 12.0), (4.0, 22.0), (1.5, 40.0)] {
        let shadow = card.expand(grow).translate(vec2(0.0, 2.0 + 4.0 * lift));
        painter.add(Shape::convex_polygon(
            outline(shadow, 7.0 + grow),
            Color32::from_black_alpha((alpha * fade * lift.clamp(0.0, 1.0)) as u8),
            Stroke::NONE,
        ));
    }
    painter.add(Shape::convex_polygon(
        outline(card, 7.0),
        palette.card.gamma_multiply(fade),
        Stroke::new(1.5_f32, palette.accent.gamma_multiply(fade)),
    ));
    let strip = Rect::from_min_max(
        card.min + vec2(6.0, 1.0),
        pos2(card.right() - 6.0, card.top() + 3.0),
    );
    painter.add(Shape::convex_polygon(
        outline(strip, 1.0),
        palette.accent.gamma_multiply(fade),
        Stroke::NONE,
    ));

    let icon_at = turn(pos2(
        card.left() + (PAD + ICON_SIZE * 0.5) * scale,
        card.center().y,
    ));
    crate::ui::paint_icon(
        &painter,
        Rect::from_center_size(icon_at, Vec2::splat(ICON_SIZE * scale)),
        tab.icon,
        palette.accent.gamma_multiply(fade),
    );
    let text = palette.text.gamma_multiply(fade);
    let galley = painter.layout_no_wrap(
        tab.title.to_owned(),
        FontId::new(font.size * scale, font.family.clone()),
        text,
    );
    let mut title = TextShape::new(
        turn(pos2(
            card.left() + (PAD + ICON_ROOM) * scale,
            card.center().y - galley.size().y * 0.5,
        )),
        galley,
        text,
    );
    title.angle = lean;
    painter.add(title);
    // A carried tab is always on the move; keep the frames coming.
    ctx.request_repaint();
}

fn ghost_gone(ctx: &Context) {
    let _ = since_on(ctx, Id::new(GHOST_ID).with("lift"), false);
}

// --------------------------------------------------------------- overlay

const OVERLAY_ID: &str = "q0editor.dock.overlay";

#[derive(Clone, Default)]
struct OverlayMemory {
    /// What was shown last, so it can fade out after the tab is let go.
    last: Option<Overlay>,
    last_target: Option<(Rect, DropKind)>,
    /// The leaf the buttons popped in over, and when.
    over: Option<Rect>,
    popped: f64,
    /// Where the last tab landed, and when.
    flash: Option<(Rect, f64)>,
}

/// The drop overlay: the leaf under the pointer, its drop buttons, the
/// place the tab would land, and rails along the workspace's edges.
fn paint_overlay(
    ctx: &Context,
    palette: &Palette,
    overlay: Option<&Overlay>,
    target: Option<(Rect, DropKind)>,
    carrying_in: Option<Rect>,
    now: f64,
) {
    let id = Id::new(OVERLAY_ID);
    let mut memory: OverlayMemory = ctx.data(|data| data.get_temp(id)).unwrap_or_default();
    let showing = overlay.is_some() || target.is_some();
    let presence = ctx.animate_bool_with_time(id.with("presence"), showing, 0.14);
    if since_on(ctx, id.with("began"), showing).is_some_and(|since| since <= 0.0) {
        // A new carry: the highlight and the target start where they are.
        if let Some(rect) = overlay.and_then(|overlay| overlay.highlight) {
            place_rect(ctx, id.with("highlight"), rect);
        }
        if let Some((rect, _)) = target {
            place_rect(ctx, id.with("target"), rect);
        }
    }
    if ctx.input(|input| input.pointer.any_released()) {
        if let Some((rect, _)) = target {
            memory.flash = Some((rect, now));
        }
    }
    if showing {
        memory.last = overlay.cloned();
        memory.last_target = target;
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, id));

    if let Some(bounds) = carrying_in {
        paint_rails(ctx, &painter, palette, bounds);
    }

    if presence > 0.01 {
        let overlay = overlay.or(memory.last.as_ref());
        if let Some(rect) = overlay.and_then(|overlay| overlay.highlight) {
            let shown = glide_rect(ctx, id.with("highlight"), rect, 0.12);
            painter.rect_filled(shown, 8.0, palette.accent.gamma_multiply(0.04 * presence));
            painter.rect_stroke(
                shown.shrink(1.0),
                8.0,
                Stroke::new(1.5_f32, palette.accent.gamma_multiply(0.4 * presence)),
            );
            let moved = match memory.over {
                Some(over) => (over.center() - rect.center()).length() > 2.0,
                None => true,
            };
            if moved {
                memory.over = Some(rect);
                memory.popped = now;
            }
        }
        let target = target.or(memory.last_target);
        if let Some((rect, kind)) = target {
            let shown = glide_rect(ctx, id.with("target"), rect, 0.09);
            paint_target(&painter, palette, shown, kind, presence, now);
            ctx.request_repaint();
        }
        if let Some(overlay) = overlay {
            let since = (now - memory.popped) as f32;
            for (order, button) in ordered(&overlay.buttons).into_iter().enumerate() {
                let pop = ease_back(progress(ctx, since - order as f32 * 0.035, 0.22));
                let hot = ctx.animate_bool_with_time(
                    id.with(("button", split_key(button.split))),
                    button.hovered && showing,
                    0.1,
                );
                paint_drop_button(&painter, palette, button, pop, hot, presence);
            }
        }
    }

    if let Some((rect, at)) = memory.flash {
        let t = progress(ctx, (now - at) as f32, 0.42);
        if t < 1.0 {
            let spread = ease_out(t);
            let fade = 1.0 - t;
            painter.rect_filled(rect, 8.0, palette.accent.gamma_multiply(0.22 * fade * fade));
            painter.rect_stroke(
                rect.expand(2.0 + 8.0 * spread),
                8.0 + 4.0 * spread,
                Stroke::new(3.0 * fade, palette.accent.gamma_multiply(fade)),
            );
        } else {
            memory.flash = None;
        }
    }
    ctx.data_mut(|data| data.insert_temp(id, memory));
}

fn split_key(split: Option<Split>) -> u8 {
    match split {
        None => 0,
        Some(Split::Left) => 1,
        Some(Split::Above) => 2,
        Some(Split::Right) => 3,
        Some(Split::Below) => 4,
    }
}

/// The buttons in the order they pop in: the middle one, then round from
/// the left.
fn ordered(buttons: &[DropButton]) -> Vec<&DropButton> {
    let mut ordered: Vec<&DropButton> = buttons.iter().collect();
    ordered.sort_by_key(|button| split_key(button.split));
    ordered
}

/// Where a carried tab would land.
fn paint_target(
    painter: &Painter,
    palette: &Palette,
    rect: Rect,
    kind: DropKind,
    presence: f32,
    now: f64,
) {
    let accent = palette.accent;
    match kind {
        DropKind::Dock => {
            let breathe = 0.85 + 0.15 * ((now * 5.0).sin() as f32);
            let area = rect.shrink(2.0);
            painter.rect_filled(area, 8.0, accent.gamma_multiply(0.14 * presence * breathe));
            painter.rect_stroke(
                area.shrink(3.0),
                6.0,
                Stroke::new(4.0_f32, accent.gamma_multiply(0.1 * presence)),
            );
            painter.rect_stroke(
                area,
                8.0,
                Stroke::new(2.0_f32, accent.gamma_multiply(0.9 * presence)),
            );
        }
        DropKind::Tab => {
            // A caret where the tab would go in among the others.
            painter.rect_filled(
                rect.shrink2(vec2(1.0, 3.0)),
                4.0,
                accent.gamma_multiply(0.1 * presence),
            );
            let x = rect.left() + 1.0;
            let (top, bottom) = (rect.top() + 3.0, rect.bottom() - 2.0);
            painter.rect_filled(
                Rect::from_min_max(pos2(x - 3.5, top), pos2(x + 3.5, bottom)),
                3.5,
                accent.gamma_multiply(0.18 * presence),
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(x - 1.25, top), pos2(x + 1.25, bottom)),
                1.25,
                accent.gamma_multiply(presence),
            );
            for (y, way) in [(top, 1.0), (bottom, -1.0)] {
                painter.add(Shape::convex_polygon(
                    vec![pos2(x - 4.0, y), pos2(x + 4.0, y), pos2(x, y + 4.0 * way)],
                    accent.gamma_multiply(presence),
                    Stroke::NONE,
                ));
            }
        }
        DropKind::Window => {
            painter.rect_filled(rect, 8.0, accent.gamma_multiply(0.05 * presence));
            dashed_outline(
                painter,
                rect,
                8.0,
                Stroke::new(1.6_f32, accent.gamma_multiply(0.85 * presence)),
                (now * 30.0) as f32,
            );
        }
    }
}

/// A drop button: a tile with a little panel on it, the part the tab would
/// take filled in. `pop` is how far it has popped in, `hot` how much the
/// pointer is on it.
fn paint_drop_button(
    painter: &Painter,
    palette: &Palette,
    button: &DropButton,
    pop: f32,
    hot: f32,
    presence: f32,
) {
    if pop <= 0.01 {
        return;
    }
    let tile = Rect::from_center_size(
        button.rect.center(),
        button.rect.size() * pop * (1.0 + 0.12 * hot),
    );
    let fade = presence * pop.min(1.0);
    painter.rect_filled(
        tile.translate(vec2(0.0, 2.0 + 2.0 * hot)).expand(1.5),
        9.0,
        Color32::from_black_alpha((60.0 * fade) as u8),
    );
    painter.rect_filled(tile, 8.0, palette.tile.gamma_multiply(0.95 * fade));
    painter.rect_stroke(
        tile,
        8.0,
        Stroke::new(
            1.0 + 0.6 * hot,
            mix(palette.line, palette.accent, 0.35 + 0.65 * hot).gamma_multiply(fade),
        ),
    );
    let frame = tile.shrink(tile.width() * 0.22);
    let ink = mix(palette.text_dim, palette.text, hot).gamma_multiply(fade);
    let fill = palette.accent.gamma_multiply((0.4 + 0.6 * hot) * fade);
    let (min, max, middle) = (frame.min, frame.max, frame.center());
    let part = match button.split {
        None => frame,
        Some(Split::Left) => Rect::from_min_max(min, pos2(middle.x, max.y)),
        Some(Split::Right) => Rect::from_min_max(pos2(middle.x, min.y), max),
        Some(Split::Above) => Rect::from_min_max(min, pos2(max.x, middle.y)),
        Some(Split::Below) => Rect::from_min_max(pos2(min.x, middle.y), max),
    };
    painter.rect_filled(part.shrink(2.0), 2.0, fill);
    painter.rect_stroke(frame, 3.0, Stroke::new(1.2_f32, ink));
    match button.split {
        None => {
            // A tab on top: this one goes in among the tabs.
            let notch = Rect::from_min_max(
                pos2(frame.left() + 1.0, frame.top() - 3.5),
                pos2(frame.left() + frame.width() * 0.45, frame.top() + 0.5),
            );
            painter.rect_filled(
                notch,
                Rounding {
                    nw: 2.0,
                    ne: 2.0,
                    sw: 0.0,
                    se: 0.0,
                },
                ink,
            );
        }
        Some(split) => {
            let line = if matches!(split, Split::Left | Split::Right) {
                [pos2(middle.x, min.y + 1.0), pos2(middle.x, max.y - 1.0)]
            } else {
                [pos2(min.x + 1.0, middle.y), pos2(max.x - 1.0, middle.y)]
            };
            painter.line_segment(line, Stroke::new(1.0_f32, ink));
        }
    }
}

/// Rails along the workspace's edges while a tab is carried, brighter the
/// nearer the pointer: a tab let go right at an edge docks along all of it.
fn paint_rails(ctx: &Context, painter: &Painter, palette: &Palette, bounds: Rect) {
    let Some(pointer) = ctx.input(|input| input.pointer.latest_pos()) else {
        return;
    };
    if !bounds.contains(pointer) {
        return;
    }
    let sides = [
        (pointer.x - bounds.left(), Split::Left),
        (bounds.right() - pointer.x, Split::Right),
        (pointer.y - bounds.top(), Split::Above),
        (bounds.bottom() - pointer.y, Split::Below),
    ];
    for (distance, side) in sides {
        let near = (1.0 - distance / 120.0).clamp(0.0, 1.0);
        let near = ctx.animate_value_with_time(
            Id::new(OVERLAY_ID).with(("rail", split_key(Some(side)))),
            near * near,
            0.08,
        );
        if near <= 0.01 {
            continue;
        }
        let (rail, along) = match side {
            Split::Left => (
                Rect::from_min_max(
                    pos2(bounds.left() + 2.0, bounds.top() + 6.0),
                    pos2(bounds.left() + 5.0, bounds.bottom() - 6.0),
                ),
                pointer.y,
            ),
            Split::Right => (
                Rect::from_min_max(
                    pos2(bounds.right() - 5.0, bounds.top() + 6.0),
                    pos2(bounds.right() - 2.0, bounds.bottom() - 6.0),
                ),
                pointer.y,
            ),
            Split::Above => (
                Rect::from_min_max(
                    pos2(bounds.left() + 6.0, bounds.top() + 2.0),
                    pos2(bounds.right() - 6.0, bounds.top() + 5.0),
                ),
                pointer.x,
            ),
            Split::Below => (
                Rect::from_min_max(
                    pos2(bounds.left() + 6.0, bounds.bottom() - 5.0),
                    pos2(bounds.right() - 6.0, bounds.bottom() - 2.0),
                ),
                pointer.x,
            ),
        };
        painter.rect_filled(
            rail.expand(3.0),
            4.5,
            palette.accent.gamma_multiply(0.1 * near),
        );
        painter.rect_filled(rail, 1.5, palette.accent.gamma_multiply(0.45 * near));
        // Brightest across from the pointer.
        let reach = 90.0;
        let bright = if matches!(side, Split::Left | Split::Right) {
            Rect::from_min_max(
                pos2(rail.left(), (along - reach).max(rail.top())),
                pos2(rail.right(), (along + reach).min(rail.bottom())),
            )
        } else {
            Rect::from_min_max(
                pos2((along - reach).max(rail.left()), rail.top()),
                pos2((along + reach).min(rail.right()), rail.bottom()),
            )
        };
        painter.rect_filled(bright, 1.5, palette.accent.gamma_multiply(near));
    }
}

/// A rounded rectangle's outline in dashes that run round it as `phase`
/// grows.
fn dashed_outline(painter: &Painter, rect: Rect, radius: f32, stroke: Stroke, phase: f32) {
    let mut points = Vec::new();
    rounded_rectangle(&mut points, rect, Rounding::same(radius));
    if points.len() < 2 {
        return;
    }
    points.push(points[0]);
    let (dash, gap) = (5.0, 4.0);
    let period = dash + gap;
    let mut walked = phase.rem_euclid(period);
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let length = from.distance(to);
        if length <= 0.0 {
            continue;
        }
        let mut at = 0.0;
        while at < length {
            let in_period = (walked + at).rem_euclid(period);
            let step = if in_period < dash {
                (dash - in_period).min(length - at)
            } else {
                (period - in_period).min(length - at)
            };
            if in_period < dash {
                let a = from + (to - from) * (at / length);
                let b = from + (to - from) * ((at + step) / length);
                painter.line_segment([a, b], stroke);
            }
            at += step.max(0.01);
        }
        walked += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        Palette {
            accent: Color32::from_rgb(90, 140, 255),
            text: Color32::WHITE,
            text_dim: Color32::GRAY,
            card: Color32::from_gray(40),
            line: Color32::from_gray(20),
            hover: Color32::from_gray(60),
            tile: Color32::from_gray(30),
        }
    }

    fn tab(index: usize, active: bool, carried: Option<Pos2>) -> Tab {
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 26.0));
        Tab {
            key: Id::new(("tab", index)),
            icon: Icon::Library,
            title: "Library",
            leaf: Id::new("leaf"),
            rect: Rect::from_min_size(pos2(index as f32 * 100.0, 0.0), vec2(100.0, 26.0)),
            bar,
            layer: LayerId::background(),
            clip: bar,
            hovered: index == 1,
            pressed: false,
            close: Some(Close {
                rect: Rect::from_min_size(pos2(index as f32 * 100.0 + 76.0, 1.0), vec2(24.0, 24.0)),
                hovered: false,
                pressed: false,
            }),
            active,
            focused: active,
            carried,
        }
    }

    #[test]
    fn the_dock_draws_through_a_carry_and_a_landing() {
        let ctx = Context::default();
        let overlay = Overlay {
            highlight: Some(Rect::from_min_size(pos2(0.0, 30.0), vec2(400.0, 300.0))),
            buttons: [None, Some(Split::Left), Some(Split::Right)]
                .into_iter()
                .map(|split| DropButton {
                    rect: Rect::from_center_size(pos2(200.0, 180.0), vec2(40.0, 40.0)),
                    split,
                    hovered: split.is_none(),
                })
                .collect(),
            target: Some(egui_dock::q0s::DropTarget {
                rect: Rect::from_min_size(pos2(0.0, 30.0), vec2(200.0, 300.0)),
                kind: DropKind::Dock,
            }),
        };
        let mut painted = 0;
        for frame in 0..6 {
            let carrying = (1..4).contains(&frame);
            let released = frame == 4;
            let output = ctx.run(
                egui::RawInput {
                    time: Some(frame as f64 * 0.05),
                    events: vec![
                        egui::Event::PointerMoved(pos2(150.0 + frame as f32 * 10.0, 120.0)),
                        egui::Event::PointerButton {
                            pos: pos2(150.0, 120.0),
                            button: egui::PointerButton::Primary,
                            pressed: !released,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    paint(
                        ctx,
                        Shown {
                            palette: palette(),
                            tabs: vec![
                                tab(0, true, None),
                                tab(1, false, carrying.then_some(pos2(120.0, 10.0))),
                                tab(2, false, None),
                            ],
                            overlay: (carrying || released).then(|| overlay.clone()),
                            edge: None,
                            bounds: Rect::from_min_size(Pos2::ZERO, vec2(400.0, 340.0)),
                        },
                    );
                },
            );
            painted += output.shapes.len();
        }
        assert!(painted > 0, "the dock drew nothing");
    }

    #[test]
    fn drop_buttons_pop_in_from_the_middle_then_round_from_the_left() {
        let button = |split| DropButton {
            rect: Rect::NOTHING,
            split,
            hovered: false,
        };
        let buttons = [
            button(Some(Split::Below)),
            button(Some(Split::Right)),
            button(None),
            button(Some(Split::Left)),
        ];
        let order: Vec<_> = ordered(&buttons).iter().map(|b| b.split).collect();
        assert_eq!(
            order,
            [
                None,
                Some(Split::Left),
                Some(Split::Right),
                Some(Split::Below)
            ]
        );
    }

    #[test]
    fn a_glide_settles_on_its_target_and_a_placed_one_leaves_from_there() {
        let ctx = Context::default();
        let id = Id::new("glide");
        let mut value = 0.0;
        for frame in 0..60 {
            ctx.begin_frame(egui::RawInput {
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            });
            if frame == 0 {
                place(&ctx, id, 0.0);
            }
            value = glide(&ctx, id, 100.0, 0.2).0;
            if frame == 1 {
                assert!(value > 0.0 && value < 100.0, "it should start on its way");
            }
            let _ = ctx.end_frame();
        }
        assert!((value - 100.0).abs() < 0.5, "settled at {value}");
    }
}
