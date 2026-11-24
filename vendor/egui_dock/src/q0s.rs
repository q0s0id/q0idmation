//! q0s: hooks that let the app draw the dock its own way.
//!
//! The dock still lays out tabs, takes their clicks and drags, and works out
//! where a dragged tab would land; the app gets told what it would have
//! drawn and draws it, animated, itself.
//!
//! - Tab buttons: [`TabViewer::paints_tabs`](crate::TabViewer::paints_tabs)
//!   turns the dock's own drawing off, and
//!   [`TabViewer::paint_tab`](crate::TabViewer::paint_tab) gets a
//!   [`TabLook`] for each one.
//! - The drop overlay: with
//!   [`OverlayStyle::q0s_custom`](crate::OverlayStyle::q0s_custom) set, the
//!   dock draws none of it, and [`take_overlay`] hands over what it would
//!   have drawn this frame.

use egui::{Context, Id, Pos2, Rect, Response};

use crate::Split;

/// One tab button as the dock laid it out this frame.
pub struct TabLook<'a> {
    /// The leaf the tab sits in; the same from frame to frame while the
    /// layout holds.
    pub leaf: Id,
    /// Its place among the leaf's tabs.
    pub index: usize,
    /// How many tabs the leaf has.
    pub count: usize,
    /// Where the button is in its bar.
    pub rect: Rect,
    /// The whole tab bar it sits in.
    pub bar: Rect,
    /// The button's own response: hover, press, clicks.
    pub response: &'a Response,
    /// Its close button, when it has one.
    pub close: Option<&'a Response>,
    /// The leaf shows this tab.
    pub active: bool,
    /// ...and the leaf is the one last used.
    pub focused: bool,
    /// The tab has been picked up and is being carried: where the pointer
    /// took hold of it.
    pub carried: Option<Pos2>,
}

/// What the drop overlay shows while a tab is carried.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    /// The leaf or window under the pointer.
    pub highlight: Option<Rect>,
    /// The drop buttons over it: where each is, which way it splits
    /// (`None`: into the leaf as a tab), and whether the pointer is on it.
    pub buttons: Vec<DropButton>,
    /// Where the tab would land if let go now.
    pub target: Option<DropTarget>,
}

/// One of the drop buttons over the hovered leaf.
#[derive(Clone, Copy, Debug)]
pub struct DropButton {
    /// Where it is.
    pub rect: Rect,
    /// Which way it splits the leaf; `None` puts the tab in among its tabs.
    pub split: Option<Split>,
    /// The pointer is on it.
    pub hovered: bool,
}

/// What kind of place a carried tab would land in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropKind {
    /// Docked: into a leaf, or split off beside one.
    Dock,
    /// Among the tabs of a bar, before the one at `rect`.
    Tab,
    /// Floating, in a window of its own.
    Window,
}

/// Where a carried tab would land.
#[derive(Clone, Copy, Debug)]
pub struct DropTarget {
    /// The area it would take, or for [`DropKind::Tab`] the tab it would
    /// go in front of.
    pub rect: Rect,
    /// What kind of place that is.
    pub kind: DropKind,
}

fn overlay_id() -> Id {
    Id::new("q0s_dock_overlay")
}

pub(crate) fn record(ctx: &Context, note: impl FnOnce(&mut Overlay)) {
    ctx.data_mut(|data| note(data.get_temp_mut_or_default::<Overlay>(overlay_id())));
}

/// What the drop overlay showed this frame, if a tab is being carried and
/// the style asks the app to draw it.
pub fn take_overlay(ctx: &Context) -> Option<Overlay> {
    ctx.data_mut(|data| data.remove_temp::<Overlay>(overlay_id()))
}
