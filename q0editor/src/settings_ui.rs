//! The Settings window. A sidebar lists the pages; each page is a column of
//! rows laid out like the keyboard shortcut editor's: the setting's name on
//! the left, its control on the right, a hairline under it. Every change is
//! saved as it is made, so there is nothing to apply or cancel.

use egui::{pos2, vec2, Context, Id, Margin, Rect, Response, RichText, Rounding, Sense, Ui};

use crate::app::{Action, EditorApp};
use crate::cursors::{CursorKind, CursorSettings};
use crate::settings::{ColorRgb, PersistFontSize, Theme};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    General,
    Appearance,
    Drawing,
    Timeline,
    Online,
    Files,
    Scripting,
    Shortcuts,
}

impl Page {
    const ALL: [Self; 8] = [
        Self::General,
        Self::Appearance,
        Self::Drawing,
        Self::Timeline,
        Self::Online,
        Self::Files,
        Self::Scripting,
        Self::Shortcuts,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Drawing => "Drawing & tools",
            Self::Timeline => "Timeline",
            Self::Online => "Online",
            Self::Files => "Files & autosave",
            Self::Scripting => "Script editor",
            Self::Shortcuts => "Shortcuts",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::General => "Extra tools, and how frames reach the screen.",
            Self::Appearance => {
                "Make it yours. Pick a palette or tune each colour; every panel follows."
            }
            Self::Drawing => "How selections move and how pencil lines settle.",
            Self::Timeline => "Sizes, motion, onion skin and the parenting column.",
            Self::Online => "What other people see of you while q0editor is open.",
            Self::Files => "Recovery snapshots, recent projects and file types.",
            Self::Scripting => "The q0lang script editor (View > Script Editor, F9).",
            Self::Shortcuts => "Click a key to change it, + to add another. Esc cancels.",
        }
    }

    /// Words a search finds this page by, besides its name.
    fn keywords(self) -> &'static str {
        match self {
            Self::General => {
                "physics particles light lights extra tools vsync vertical sync display stutter refresh restart fps frame rate counter frames per second performance lag"
            }
            Self::Appearance => {
                "theme colour color palette preset q7s gradient canvas photo stage border background onion tint syntax accent dark light window cursor cursors pointer mouse arrow animated animation wrap number drag blender squash stretch subselect library preview checkerboard checker interface ui size scale zoom resolution dpi bigger smaller"
            }
            Self::Drawing => "arrow keys nudge move selection shift angle turn snap reach pick lines click pencil join ends close select drawn back to select rotate rotation handle grip corner turn size brush eraser",
            Self::Timeline => {
                "frame width row height layer names column motion animation animated glide playhead keyframe key corners round rounded rounding square pill dot dots shine highlight gloss flat onion skin playback lights parenting parent"
            }
            Self::Online => "discord presence profile application id",
            Self::Files => {
                "autosave recovery snapshot crash recent projects file association register q1s q0l q0lang windows"
            }
            Self::Scripting => "q0lang script code font size tab width line numbers",
            Self::Shortcuts => "keyboard keys hotkeys keybinds bindings",
        }
    }

    fn matches(self, query: &str) -> bool {
        query.is_empty()
            || self.label().to_lowercase().contains(query)
            || self.keywords().contains(query)
    }
}

const PAGE_ID: &str = "settings_page";
const SEARCH_ID: &str = "settings_search";
const NAV_WIDTH: f32 = 184.0;

pub(crate) fn render(app: &mut EditorApp, ctx: &Context) {
    let page_id = Id::new(PAGE_ID);
    let search_id = Id::new(SEARCH_ID);
    let mut page = ctx
        .data(|d| d.get_temp::<Page>(page_id))
        .unwrap_or_default();
    let mut search = ctx
        .data(|d| d.get_temp::<String>(search_id))
        .unwrap_or_default();
    let screen = ctx.screen_rect();
    let mut open = true;
    // No `fit_to_screen`: the page scrolls by itself, and a scrolling body
    // would give the sidebar no bottom to stand on.
    crate::dialog::Dialog::new("Settings")
        .open(&mut open)
        .default_size([780.0, 560.0])
        .min_size([560.0, 340.0])
        .max_height((screen.height() - 110.0).max(320.0))
        .default_pos(screen.center() - vec2(390.0, 300.0))
        .collapsible(false)
        .resizable(true)
        .inner_margin(Margin::ZERO)
        .constrain_to(screen)
        .show(ctx, |ui| {
            let full = ui.available_rect_before_wrap();
            ui.allocate_rect(full, Sense::hover());
            let nav = Rect::from_min_max(full.min, pos2(full.left() + NAV_WIDTH, full.bottom()));
            let body = Rect::from_min_max(pos2(nav.right(), full.top()), full.max);
            paint_nav_backdrop(ui, nav);
            let clip = ui.clip_rect();
            let mut nav_ui = ui.child_ui_with_id_source(
                nav.shrink2(vec2(10.0, 12.0)),
                egui::Layout::top_down(egui::Align::Min),
                "settings_nav",
            );
            nav_ui.set_clip_rect(nav.intersect(clip));
            sidebar(&mut nav_ui, &mut page, &mut search);
            let mut page_ui = ui.child_ui_with_id_source(
                Rect::from_min_max(body.min + vec2(22.0, 16.0), body.max - vec2(14.0, 12.0)),
                egui::Layout::top_down(egui::Align::Min),
                "settings_page",
            );
            page_ui.set_clip_rect(body.intersect(clip));
            show_page(app, &mut page_ui, &mut page);
        });
    if !open {
        app.session.show_settings = false;
    }
    // A key being captured on the Shortcuts page stops when the page goes,
    // not one a keycap elsewhere is waiting for.
    let editing_here = app.session.show_settings && page == Page::Shortcuts;
    let shown_id = Id::new("settings_shortcuts_shown");
    let was_editing_here = ctx.data(|d| d.get_temp::<bool>(shown_id).unwrap_or(false));
    if was_editing_here && !editing_here && !app.session.show_shortcuts {
        app.session.shortcut_capture = None;
        app.session.shortcut_feedback = None;
    }
    ctx.data_mut(|d| d.insert_temp(shown_id, editing_here));
    ctx.data_mut(|d| {
        d.insert_temp(page_id, page);
        if app.session.show_settings {
            d.insert_temp(search_id, search);
        } else {
            d.remove::<String>(search_id);
        }
    });
    crate::ui::hidden_palette_note(
        ctx,
        app.session.show_settings,
        app.settings.theme.accent.to_color32(),
    );
}

fn paint_nav_backdrop(ui: &Ui, rect: Rect) {
    let visuals = ui.visuals();
    // Follow the window's rounded corner at the bottom left.
    let radius = (visuals.window_rounding.sw - visuals.window_stroke.width * 0.5).max(0.0);
    ui.painter().rect_filled(
        rect,
        Rounding {
            sw: radius,
            ..Rounding::ZERO
        },
        visuals.extreme_bg_color.gamma_multiply(0.45),
    );
    ui.painter().vline(
        rect.right(),
        rect.y_range(),
        visuals.widgets.noninteractive.bg_stroke,
    );
}

fn sidebar(ui: &mut Ui, page: &mut Page, search: &mut String) {
    crate::ui::search_field(ui, search, "Search settings");
    ui.add_space(8.0);
    let query = search.trim().to_lowercase();
    let shown: Vec<Page> = Page::ALL
        .into_iter()
        .filter(|item| item.matches(&query))
        .collect();
    if let Some(&first) = shown.first() {
        if !shown.contains(page) {
            *page = first;
        }
    }
    let width = ui.available_width();
    ui.spacing_mut().item_spacing.y = 2.0;
    for item in shown.iter().copied() {
        if nav_button(ui, item.label(), item == *page, width).clicked() {
            *page = item;
        }
    }
    if shown.is_empty() {
        ui.weak("Nothing matches");
    }
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.label(
            RichText::new("Changes save as you make them.")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    });
}

/// A page in the sidebar, worn like the chosen tool when it is open.
fn nav_button(ui: &mut Ui, label: &str, selected: bool, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, label)
    });
    if ui.is_rect_visible(rect) {
        crate::ui::paint_control(ui, &response, rect, 6.0, selected);
        let color = if selected {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        };
        crate::ui::clipped_label(
            ui.painter(),
            rect.shrink2(vec2(10.0, 0.0)),
            label,
            egui::FontId::proportional(13.5),
            color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn show_page(app: &mut EditorApp, ui: &mut Ui, page: &mut Page) {
    ui.label(RichText::new(page.label()).size(20.0).strong());
    ui.label(RichText::new(page.detail()).color(ui.visuals().weak_text_color()));
    ui.add_space(4.0);
    ui.separator();
    if *page == Page::Shortcuts {
        // The editor scrolls its own list under its search and categories.
        if crate::panels::shortcuts::editor(app, ui) {
            app.settings.save();
        }
        return;
    }
    egui::ScrollArea::vertical()
        .id_source(("settings_page", *page as u8))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Room for the scroll bar, which floats over the right edge.
            ui.set_max_width(ui.available_width() - 10.0);
            match *page {
                Page::General => general(app, ui),
                Page::Appearance => appearance(app, ui),
                Page::Drawing => drawing(app, ui),
                Page::Timeline => timeline(app, ui, page),
                Page::Online => online(app, ui),
                Page::Files => files(app, ui),
                Page::Scripting => scripting(app, ui, page),
                Page::Shortcuts => {}
            }
            ui.add_space(12.0);
        });
}

// ---------------------------------------------------------------------------
// Rows

/// Opens a group of rows, the way the shortcut editor opens a category.
fn group(ui: &mut Ui, title: &str, detail: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).strong());
    if !detail.is_empty() {
        note(ui, detail);
    }
    ui.add_space(2.0);
}

/// One setting: its name on the left, `control` on the right, a hairline
/// under both. Returns the name's response too, for its hover text.
fn row<R>(
    ui: &mut Ui,
    name: &str,
    hint: &str,
    control: impl FnOnce(&mut Ui) -> R,
) -> (Response, R) {
    let height = ui.spacing().interact_size.y;
    let name_width = (ui.available_width() * 0.4).clamp(140.0, 250.0);
    let (label, inner) = ui
        .horizontal(|ui| {
            let (rect, label) = ui.allocate_exact_size(vec2(name_width, height), Sense::hover());
            crate::ui::clipped_label(
                ui.painter(),
                rect,
                name,
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().text_color(),
            );
            let label = if hint.is_empty() {
                label
            } else {
                label.on_hover_text(hint)
            };
            (label, ui.horizontal_wrapped(control).inner)
        })
        .inner;
    ui.separator();
    (label, inner)
}

/// A row with an on/off switch. True when it was flipped.
fn toggle_row(ui: &mut Ui, name: &str, hint: &str, value: &mut bool) -> bool {
    let (_, switch) = row(ui, name, hint, |ui| crate::ui::toggle_switch(ui, value));
    if hint.is_empty() {
        switch.changed()
    } else {
        switch.on_hover_text(hint).changed()
    }
}

fn note(ui: &mut Ui, text: impl Into<String>) {
    ui.label(
        RichText::new(text.into())
            .small()
            .color(ui.visuals().weak_text_color()),
    );
}

/// Whether an edit has come to rest: typed in, or let go after a drag.
/// Saving on every frame of a drag would write the file each frame.
fn settled(response: &Response) -> bool {
    response.drag_stopped() || (response.changed() && !response.dragged())
}

fn save_or_report(app: &mut EditorApp, what: &str) {
    if let Err(error) = app.settings.try_save() {
        app.session.status = format!("could not save {what}: {error}");
    }
}

/// A note sending the reader to another page, with a button that goes there.
fn see_also(ui: &mut Ui, page: &mut Page, text: &str, to: Page) {
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        note(ui, text);
        if ui.small_button(format!("Open {}", to.label())).clicked() {
            *page = to;
        }
    });
}

// ---------------------------------------------------------------------------
// Pages

fn general(app: &mut EditorApp, ui: &mut Ui) {
    group(
        ui,
        "Extra tools",
        "Bigger features that stay out of sight until you turn them on.",
    );
    if toggle_row(
        ui,
        "Physics editor",
        "Objects fall, bounce and collide, and the result is baked into keyframes. Adds Modify > Physics and a Physics button.",
        &mut app.settings.physics_enabled,
    ) {
        if !app.settings.physics_enabled {
            app.session.physics.open = false;
            app.session.physics.stop_preview();
        }
        save_or_report(app, "the physics preference");
    }
    if toggle_row(
        ui,
        "Particle system",
        "Emitters spray fire, smoke, sparks, snow or library items, baked into keyframes. Adds Modify > Particles and a Particles button.",
        &mut app.settings.particles_enabled,
    ) {
        if !app.settings.particles_enabled {
            app.session.particles.open = false;
            app.session.particles.stop_preview();
        }
        save_or_report(app, "the particles preference");
    }
    if toggle_row(
        ui,
        "Light editor",
        "Real lights: darken the scene and drop lamps, torches and spotlights on it. Nothing is baked; exports and q0player light every frame. Adds Modify > Light and a Light button.",
        &mut app.settings.light_enabled,
    ) {
        if !app.settings.light_enabled {
            app.session.lights.open = false;
        }
        save_or_report(app, "the light preference");
    }

    group(ui, "Display", "Frame presentation and responsiveness.");
    if toggle_row(
        ui,
        "Vertical sync (VSync)",
        "Synchronize frame presentation with display refresh. Try disabling this if movement stutters despite low frame processing times. Applies after restarting q0editor.",
        &mut app.settings.vertical_sync,
    ) {
        app.session.status = match app.settings.try_save() {
            Ok(()) => "display synchronization changed; restart q0editor to apply".into(),
            Err(error) => format!("could not save display preference: {error}"),
        };
    }
    if app
        .perf
        .presentation_sync()
        .is_some_and(|active| active != app.settings.vertical_sync)
    {
        note(ui, "Restart q0editor to apply the display setting.");
    }
    if toggle_row(
        ui,
        "Frame rate counter",
        "Shows in the stage's corner how many frames a second the editor really draws, how long each takes to work out, and the last second as bars. The editor draws only when something changes, so it reads idle when nothing does.",
        &mut app.settings.show_fps,
    ) {
        save_or_report(app, "the frame rate counter");
    }
}

fn appearance(app: &mut EditorApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    group(
        ui,
        "Theme",
        "Choose a palette. Every panel follows your theme.",
    );
    ui.horizontal_wrapped(|ui| {
        for preset in Theme::PRESETS {
            let selected = app.settings.theme.accent == preset.accent.into()
                && app.settings.theme.window == preset.window.into()
                && app.settings.theme.panel == preset.panel.into();
            if crate::theme::preset_button(ui, preset, selected).clicked() {
                app.settings.theme = Theme::from_builtin(preset);
                app.settings.theme.apply(&ctx);
                app.settings.save();
                app.session.status = format!("theme: {}", preset.name);
            }
        }
    });
    ui.add_space(8.0);
    ui.separator();
    row(
        ui,
        "Interface size",
        "Draws all of q0editor bigger or smaller, as a lower or higher screen resolution would, without changing the screen",
        |ui| {
            let now = app.settings.ui_scale;
            egui::ComboBox::from_id_source("settings_ui_scale")
                .selected_text(format!("{:.0}%", now * 100.0))
                .show_ui(ui, |ui| {
                    for scale in crate::settings::UI_SCALES {
                        let label = if scale == 1.0 {
                            "100% (system)".to_string()
                        } else {
                            format!("{:.0}%", scale * 100.0)
                        };
                        if ui
                            .selectable_label((now - scale).abs() < 0.001, label)
                            .clicked()
                        {
                            app.settings.ui_scale = scale;
                            app.settings.save();
                        }
                    }
                });
        },
    );
    row(
        ui,
        "Theme file",
        "A .q7s file holds every colour here; q0player reads them too.",
        |ui| theme_file_buttons(app, ui),
    );

    let mut t = app.settings.theme.clone();
    ui.add_space(8.0);
    egui::CollapsingHeader::new("Base palette")
        .default_open(true)
        .show(ui, |ui| {
            color_row(ui, "Accent", &mut t.accent);
            color_row(ui, "Panel", &mut t.panel);
            color_row(ui, "Window", &mut t.window);
            color_row(ui, "Inputs background", &mut t.deep_bg);
            color_row(ui, "Text", &mut t.text);
            color_row(ui, "Text dim", &mut t.text_dim);
            color_row(ui, "Window stroke", &mut t.stroke_dark);
        });
    egui::CollapsingHeader::new("Gradients")
        .default_open(true)
        .show(ui, |ui| crate::ui::gradient_editor(ui, &mut t));
    egui::CollapsingHeader::new("Stage & canvas")
        .default_open(true)
        .show(ui, |ui| {
            note(
                ui,
                "The stage paper stays white. This is the drawing canvas.",
            );
            color_row(ui, "Canvas (outside stage)", &mut t.canvas_bg);
            color_row(ui, "Stage border", &mut t.stage_border);
            color_row(ui, "Symbol edit mask", &mut t.symbol_edit_mask);
            color_row(ui, "Library preview (light)", &mut t.library_preview_light);
            color_row(ui, "Library preview (dark)", &mut t.library_preview_dark);
            row(
                ui,
                "Library backgrounds",
                "The backgrounds the library preview may pick to make an item read. Rule one out and it takes the next best; at least one stays on.",
                |ui| {
                    let backgrounds = &mut app.settings.library_backgrounds;
                    let before = *backgrounds;
                    ui.checkbox(&mut backgrounds.light, "Light");
                    ui.checkbox(&mut backgrounds.dark, "Dark");
                    ui.checkbox(&mut backgrounds.checker, "Checkerboard");
                    if !(backgrounds.light || backgrounds.dark || backgrounds.checker) {
                        *backgrounds = before;
                    }
                    if *backgrounds != before {
                        app.settings.save();
                    }
                },
            );
            row(ui, "Canvas photo", "", |ui| canvas_photo(app, ui, &mut t));
            row(ui, "Photo darkening", "", |ui| {
                ui.add(egui::Slider::new(&mut t.canvas_background_darkening, 0..=100).suffix("%"));
            });
        });
    egui::CollapsingHeader::new("Timeline colours")
        .default_open(false)
        .show(ui, |ui| {
            color_row(ui, "Header strip", &mut t.timeline_header);
            color_row(ui, "Grid", &mut t.timeline_grid);
            color_row(ui, "Grid (every 5)", &mut t.timeline_grid_5);
            color_row(ui, "Playhead", &mut t.playhead);
            color_row(ui, "Keyframe dot", &mut t.keyframe);
            color_row(ui, "Span fill", &mut t.extension);
            color_row(ui, "Span fill (active layer)", &mut t.extension_active);
            // Once shown here the two kinds are edited independently of
            // each other.
            t.pin_shape_tween_colors();
            color_row(ui, "Motion tween fill", &mut t.tween_fill);
            color_row(ui, "Motion tween arrow", &mut t.tween_arrow);
            if let Some(fill) = t.shape_tween_fill.as_mut() {
                color_row(ui, "Shape tween fill", fill);
            }
            if let Some(arrow) = t.shape_tween_arrow.as_mut() {
                color_row(ui, "Shape tween arrow", arrow);
            }
            color_row(ui, "Empty cell (inside)", &mut t.empty_inside);
            color_row(ui, "Empty cell (active layer)", &mut t.empty_inside_active);
            color_row(ui, "Beyond cell", &mut t.empty_beyond);
            color_row(ui, "Beyond cell (every 5)", &mut t.empty_beyond_5);
        });
    egui::CollapsingHeader::new("Onion-skin tints")
        .default_open(false)
        .show(ui, |ui| {
            color_row(ui, "Past frames", &mut t.onion_past);
            color_row(ui, "Future frames", &mut t.onion_future);
        });
    egui::CollapsingHeader::new("q0lang syntax")
        .default_open(false)
        .show(ui, |ui| {
            note(ui, "Used by the script editor (View > Script Editor, F9).");
            color_row(ui, "Keyword", &mut t.syntax_keyword);
            color_row(ui, "Built-in / library", &mut t.syntax_builtin);
            color_row(ui, "String", &mut t.syntax_string);
            color_row(ui, "Number", &mut t.syntax_number);
            color_row(ui, "Comment", &mut t.syntax_comment);
            color_row(ui, "Operator", &mut t.syntax_operator);
            color_row(ui, "Identifier", &mut t.syntax_identifier);
            color_row(ui, "Signal (X!)", &mut t.syntax_signal);
            color_row(ui, "Gutter background", &mut t.syntax_gutter_bg);
            color_row(ui, "Gutter text", &mut t.syntax_gutter_fg);
        });
    cursors(app, ui, &mut t);
    if t != app.settings.theme {
        app.settings.theme = t;
        app.settings.theme.apply(&ctx);
        app.settings.save();
    }
}

/// The editor's own pointers: on or off, how they move, their colours (which
/// live in the theme) and a switch with a live preview for each.
fn cursors(app: &mut EditorApp, ui: &mut Ui, t: &mut Theme) {
    group(
        ui,
        "Cursors",
        "The editor draws its own pointers in your theme's colours. Hover a preview to try that pointer.",
    );
    let prefs = &mut app.settings.cursors;
    let mut changed = toggle_row(
        ui,
        "Custom cursors",
        "Off, every pointer is the system's own.",
        &mut prefs.custom,
    );
    ui.add_enabled_ui(prefs.custom, |ui| {
        let packs = crate::cursors::CursorPack::ALL.map(|pack| (pack, pack.label()));
        let (_, picked) = row(ui, "Pack", prefs.pack.hint(), |ui| {
            crate::color_picker_ui::segmented(ui, 340.0, &mut prefs.pack, &packs)
        });
        changed |= picked;
        note(ui, prefs.pack.hint());
        changed |= toggle_row(
            ui,
            "Animation",
            "Pointers play when they appear, when you press and as you move, then rest, so an idle editor draws nothing extra. Off, they hold still.",
            &mut prefs.animated,
        );
        ui.add_enabled_ui(prefs.animated, |ui| {
            changed |= toggle_row(
                ui,
                "Squash at the edge",
                "A pointer that runs into the screen's edge squashes flat against it and springs back.",
                &mut prefs.squash,
            );
            changed |= toggle_row(
                ui,
                "Stretch at speed",
                "A fast-moving pointer stretches out a little along its way.",
                &mut prefs.stretch,
            );
        });
        let (_, size) = row(ui, "Size", "1 is about the size of a system cursor.", |ui| {
            ui.add(egui::Slider::new(&mut prefs.size, CursorSettings::SIZE_RANGE).fixed_decimals(2))
        });
        changed |= settled(&size);
    });
    changed |= toggle_row(
        ui,
        "Wrap number drags",
        "Drag a number field into the window's edge and the pointer comes back from the opposite edge, so the drag never runs out of room. Windows only.",
        &mut prefs.wrap_number_drags,
    );

    ui.add_space(6.0);
    ui.label(RichText::new("Colours").strong());
    cursor_colors(ui, t);

    ui.add_space(6.0);
    ui.label(RichText::new("Pointers").strong());
    note(
        ui,
        "Each one on its own. Switched off, it falls back to the system cursor.",
    );
    let theme = t.clone();
    ui.add_enabled_ui(prefs.custom, |ui| {
        changed |= cursor_rows(ui, &CursorKind::STATES, prefs, &theme);
    });

    ui.add_space(6.0);
    ui.label(RichText::new("Action cursors").strong());
    note(
        ui,
        "The editor's own, for what a click or drag will do right there. Switched off, the plain pointer for that spot shows instead.",
    );
    ui.add_enabled_ui(prefs.custom, |ui| {
        changed |= cursor_rows(ui, &CursorKind::ACTIONS, prefs, &theme);
    });

    ui.add_space(6.0);
    ui.label(RichText::new("Your own cursors").strong());
    note(
        ui,
        "Draw any pointer pixel by pixel, or start from its Retro look or an image file. Drawing one switches the pack to Your own; states you have not drawn keep the standard pointer.",
    );
    ui.add_enabled_ui(prefs.custom, |ui| {
        if crate::cursors::own::maker(ui, &mut prefs.own, &theme) {
            prefs.pack = crate::cursors::CursorPack::Own;
            changed = true;
        }
    });
    if changed {
        save_or_report(app, "the cursor settings");
    }
}

/// A switch and a live preview for each of `kinds`. True when one flipped.
fn cursor_rows(
    ui: &mut Ui,
    kinds: &[CursorKind],
    prefs: &mut CursorSettings,
    theme: &Theme,
) -> bool {
    let mut changed = false;
    for &kind in kinds {
        let mut on = prefs.state_on(kind);
        let (_, flipped) = row(ui, kind.label(), kind.hint(), |ui| {
            let switch = crate::ui::toggle_switch(ui, &mut on).on_hover_text(kind.hint());
            crate::cursors::preview(ui, kind, prefs, theme).on_hover_text(kind.hint());
            switch.changed()
        });
        if flipped {
            prefs.set_state(kind, on);
            changed = true;
        }
    }
    changed
}

/// The pointer colours. They follow the palette until one is changed.
fn cursor_colors(ui: &mut Ui, t: &mut Theme) {
    let pinned = t.cursors.is_some();
    let mut colors = t.cursor_colors();
    let before = colors;
    note(
        ui,
        if pinned {
            "Your own colours, saved with the theme and in .q7s files."
        } else {
            "Following the palette above. Change one to pick your own."
        },
    );
    color_row(ui, "Body", &mut colors.fill);
    color_row(ui, "Outline", &mut colors.outline);
    color_row(ui, "Accent", &mut colors.accent);
    row(
        ui,
        "Shadow",
        "How dark the shadow under a pointer is.",
        |ui| {
            ui.add(egui::Slider::new(&mut colors.shadow, 0..=100).suffix("%"));
        },
    );
    if colors != before {
        t.cursors = Some(colors);
    }
    if pinned {
        row(ui, "Palette", "", |ui| {
            if ui
                .button("Follow the palette")
                .on_hover_text("Drop your own pointer colours and take them from the theme again")
                .clicked()
            {
                t.cursors = None;
            }
        });
    }
}

fn theme_file_buttons(app: &mut EditorApp, ui: &mut Ui) {
    if ui
        .button("Save as... (.q7s)")
        .on_hover_text("Persist current colours as a reusable .q7s theme")
        .clicked()
    {
        if let Some(path) = crate::platform::FileDialog::new()
            .add_filter("q7s theme", &["q7s"])
            .set_file_name("my-theme.q7s")
            .save_file()
        {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Custom")
                .to_string();
            match app.settings.theme.write_q7s(&path, &name) {
                Ok(()) => {
                    crate::platform::file_written(&path);
                    app.session.status = format!("saved theme: {}", path.display())
                }
                Err(e) => app.session.status = format!("save theme failed: {e}"),
            }
        }
    }
    if ui
        .button("Load... (.q7s)")
        .on_hover_text("Apply a previously saved .q7s theme (works with q0player .q7s files too)")
        .clicked()
    {
        if let Some(action) = crate::platform::pick_file_then(
            crate::platform::FileDialog::new().add_filter("q7s theme", &["q7s"]),
            Action::LoadThemeFromPath,
        ) {
            app.queue(action);
        }
    }
}

fn canvas_photo(app: &mut EditorApp, ui: &mut Ui, t: &mut Theme) {
    if ui.button("Choose canvas photo...").clicked() {
        if let Some(path) = crate::platform::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        {
            match crate::bitmap_import::decode_bitmap_path(&path, u16::MAX) {
                Ok(_) => {
                    t.canvas_background_image = Some(path.clone());
                    app.session.status = format!(
                        "canvas photo: {}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("image")
                    );
                }
                Err(error) => {
                    app.session.status = format!("canvas photo failed: {error}");
                }
            }
        }
    }
    if let Some(path) = t.canvas_background_image.as_ref() {
        ui.label(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("selected image"),
        );
        if ui.button("Clear photo").clicked() {
            t.canvas_background_image = None;
            t.canvas_background_darkening = 0;
        }
    }
}

fn color_row(ui: &mut Ui, label: &str, target: &mut ColorRgb) {
    let (name, _) = row(ui, label, "", |ui| {
        let mut c = [target.r, target.g, target.b];
        if crate::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
            *target = ColorRgb::new(c[0], c[1], c[2]);
        }
    });
    if label == "Window stroke" {
        crate::ui::hidden_palette_note_trigger(ui, &name);
    }
}

fn drawing(app: &mut EditorApp, ui: &mut Ui) {
    group(ui, "Selection", "");
    let (_, nudged) = row(
        ui,
        "Arrow keys move by",
        "How far one press of an arrow key moves the selection, and how far with Shift held",
        |ui| {
            let range = crate::settings::NudgeSteps::RANGE;
            let nudge = &mut app.settings.nudge;
            let step = ui.add(
                egui::DragValue::new(&mut nudge.step)
                    .speed(0.1)
                    .clamp_range(range.clone())
                    .max_decimals(2)
                    .suffix(" px"),
            );
            ui.label("with Shift");
            let shift = ui.add(
                egui::DragValue::new(&mut nudge.shift_step)
                    .speed(0.1)
                    .clamp_range(range)
                    .max_decimals(2)
                    .suffix(" px"),
            );
            settled(&step) || settled(&shift)
        },
    );
    let prefs = &mut app.settings.drawing;
    let mut changed = nudged;
    changed |= row(
        ui,
        "Pick lines within",
        "How near a click must come to a thin line or an edge to pick it",
        |ui| {
            settled(
                &ui.add(
                    egui::DragValue::new(&mut prefs.pick_reach)
                        .speed(0.1)
                        .clamp_range(1.0..=12.0)
                        .max_decimals(1)
                        .suffix(" px"),
                ),
            )
        },
    )
    .1;
    changed |= row(
        ui,
        "Rotate handle size",
        "How big the grips past the transform box's corners are, the ones you drag to turn things. They sit further out as they grow, so they never cover the box.",
        |ui| {
            settled(
                &ui.add(
                    egui::Slider::new(&mut prefs.rotate_size, 1.0..=4.0)
                        .step_by(0.25)
                        .suffix("×"),
                ),
            )
        },
    )
    .1;
    changed |= toggle_row(
        ui,
        "Select what you draw",
        "A new shape or line becomes the selection, ready to move or restyle",
        &mut prefs.select_drawn,
    );
    changed |= toggle_row(
        ui,
        "Back to Select after a shape",
        "After a rectangle, oval or line is drawn, the Select tool comes back",
        &mut prefs.then_select,
    );

    group(ui, "Shift", "");
    changed |= row(
        ui,
        "Shift steps by",
        "Holding Shift turns things by this step, and points lines and pen segments along it",
        |ui| {
            let mut picked = false;
            egui::ComboBox::from_id_source("drawing_shift_angle")
                .selected_text(format!("{}°", prefs.shift_angle))
                .show_ui(ui, |ui| {
                    for angle in crate::settings::SHIFT_ANGLES {
                        picked |= ui
                            .selectable_value(&mut prefs.shift_angle, angle, format!("{angle}°"))
                            .clicked();
                    }
                });
            picked
        },
    )
    .1;

    group(ui, "Snapping", "");
    changed |= row(
        ui,
        "Snap within",
        "How near a point must come to snap to an anchor, a line end, a crossing or the stage",
        |ui| {
            settled(
                &ui.add(
                    egui::DragValue::new(&mut prefs.snap_reach)
                        .speed(0.2)
                        .clamp_range(2.0..=40.0)
                        .max_decimals(0)
                        .suffix(" px"),
                ),
            )
        },
    )
    .1;

    group(ui, "Pencil", "");
    changed |= toggle_row(
        ui,
        "Join ends that meet",
        "A pencil line that ends close to where it began closes on itself",
        &mut prefs.join_ends,
    );
    if changed {
        app.settings.save();
    }
    ui.add_space(10.0);
    note(
        ui,
        "Smoothing, the pencil's mode and the brush, eraser and shape options sit in the Properties panel while their tool is chosen.",
    );
}

fn timeline(app: &mut EditorApp, ui: &mut Ui, page: &mut Page) {
    use crate::settings::{TIMELINE_FRAME_WIDTHS, TIMELINE_ROW_HEIGHTS};
    group(
        ui,
        "Sizes",
        "The timeline's own edges change these too, when dragged.",
    );
    let mut resized = false;
    resized |= row(ui, "Frame width", "The width of one frame cell", |ui| {
        settled(
            &ui.add(
                egui::DragValue::new(&mut app.settings.timeline_frame_width)
                    .speed(0.25)
                    .clamp_range(TIMELINE_FRAME_WIDTHS)
                    .max_decimals(1)
                    .suffix(" px"),
            ),
        )
    })
    .1;
    resized |= row(
        ui,
        "Row height",
        "The height of a layer row. A layer given its own height keeps it.",
        |ui| {
            settled(
                &ui.add(
                    egui::DragValue::new(&mut app.settings.timeline_row_height)
                        .speed(0.5)
                        .clamp_range(TIMELINE_ROW_HEIGHTS)
                        .max_decimals(1)
                        .suffix(" px"),
                ),
            )
        },
    )
    .1;
    resized |= row(
        ui,
        "Layer names column",
        "The width of the column of layer names",
        |ui| {
            settled(
                &ui.add(
                    egui::DragValue::new(&mut app.settings.timeline_layer_width)
                        .clamp_range(112..=520)
                        .suffix(" px"),
                ),
            )
        },
    )
    .1;
    resized |= row(ui, "", "", |ui| {
        let reset = ui
            .button("Reset sizes")
            .on_hover_text("Back to the sizes q0editor starts with")
            .clicked();
        if reset {
            let defaults = crate::settings::Settings::default();
            app.settings.timeline_frame_width = defaults.timeline_frame_width;
            app.settings.timeline_row_height = defaults.timeline_row_height;
            app.settings.timeline_layer_width = defaults.timeline_layer_width;
        }
        reset
    })
    .1;
    if resized {
        app.settings.save();
    }

    group(ui, "Look", "");
    if toggle_row(
        ui,
        "Motion",
        "The playhead glides, rows and switches ease, new keyframes pop in. Off: everything snaps into place.",
        &mut app.settings.timeline_motion,
    ) {
        app.settings.save();
    }
    if row(
        ui,
        "Keyframe corners",
        "How round the corners of keyframe blocks are. 0 is square; turned all the way up they become pills.",
        |ui| {
            settled(
                &ui.add(
                    egui::Slider::new(&mut app.settings.timeline_key_round, 0.0..=16.0)
                        .step_by(0.5)
                        .suffix(" px"),
                ),
            )
        },
    )
    .1
    {
        app.settings.save();
    }
    if toggle_row(
        ui,
        "Shine on keyframe dots",
        "The small white highlight on filled keyframe dots. Off: plain, flat dots.",
        &mut app.settings.timeline_dot_shine,
    ) {
        app.settings.save();
    }

    group(ui, "Onion skin", "");
    if toggle_row(
        ui,
        "During playback",
        "Keep showing onion skin while the timeline plays",
        &mut app.settings.onion_skin_during_playback,
    ) {
        app.settings.save();
    }
    if toggle_row(
        ui,
        "Lights off while shown",
        "Turns the scene's lights off while onion skin shows, so ghosts are easy to see in a dark scene. Off: everything stays lit, ghosts included.",
        &mut app.settings.onion_unlit,
    ) {
        app.settings.save();
    }

    group(ui, "Layers", "");
    let mut parenting = crate::layer_parenting_ui::shown(app);
    if toggle_row(
        ui,
        "Parenting column",
        "The column in the timeline that shows and sets each layer's parent",
        &mut parenting,
    ) {
        crate::layer_parenting_ui::toggle(app);
    }
    see_also(
        ui,
        page,
        "Frame, tween and playhead colours are part of the theme.",
        Page::Appearance,
    );
}

fn online(app: &mut EditorApp, ui: &mut Ui) {
    group(
        ui,
        "Discord",
        "What your Discord profile says while q0editor is open.",
    );
    let mut changed = toggle_row(
        ui,
        "Show what I'm working on",
        "Discord shows \"Editing <project>\" and the frame you are on, while the Discord app is running on this computer.",
        &mut app.settings.discord_presence,
    );
    ui.add_enabled_ui(app.settings.discord_presence, |ui| {
        changed |= toggle_row(
            ui,
            "Name the project",
            "Off, Discord only says you are animating.",
            &mut app.settings.discord_show_project,
        );
        changed |= row(
            ui,
            "Application id",
            "The Discord application to show as. Leave it empty for q0editor's own.",
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.settings.discord_application_id)
                        .hint_text(if crate::discord::APPLICATION_ID.is_empty() {
                            "paste one"
                        } else {
                            "q0editor's own"
                        })
                        .desired_width(170.0),
                )
                .on_hover_text(
                    "The Discord application to show as. Leave it empty for q0editor's own.",
                )
                .changed()
            },
        )
        .1;
    });
    if changed {
        let _ = app.settings.try_save();
    }
}

fn files(app: &mut EditorApp, ui: &mut Ui) {
    group(
        ui,
        "Autosave",
        "Recovery snapshots of unsaved work, offered on Home after a crash.",
    );
    let mut changed = toggle_row(
        ui,
        "Keep recovery snapshots",
        "Unsaved documents are copied to a private recovery folder in the background. Your project files are only written when you save.",
        &mut app.settings.autosave_enabled,
    );
    ui.add_enabled_ui(app.settings.autosave_enabled, |ui| {
        changed |= row(ui, "Snapshot every", "", |ui| {
            let mut minutes = app.settings.autosave_interval_secs as f32 / 60.0;
            let response = ui.add(
                egui::DragValue::new(&mut minutes)
                    .speed(0.25)
                    .clamp_range(
                        crate::autosave::MIN_INTERVAL_SECS as f32 / 60.0
                            ..=crate::autosave::MAX_INTERVAL_SECS as f32 / 60.0,
                    )
                    .max_decimals(2)
                    .suffix(" min"),
            );
            if response.changed() {
                app.settings.autosave_interval_secs = ((minutes * 60.0).round() as u32).clamp(
                    crate::autosave::MIN_INTERVAL_SECS,
                    crate::autosave::MAX_INTERVAL_SECS,
                );
            }
            settled(&response)
        })
        .1;
    });
    if changed {
        save_or_report(app, "autosave preference");
    }
    let autosave_note = if let Some(error) = app.autosave.last_error() {
        Some(format!("Last snapshot failed: {error}"))
    } else {
        app.autosave
            .last_snapshot_age()
            .map(|age| format!("Last snapshot {} s ago", age.as_secs()))
    };
    if let Some(text) = autosave_note {
        note(ui, text);
    }

    group(
        ui,
        "Recent projects",
        "Listed on Home and under File > Open Recent.",
    );
    let count = app.settings.recent_projects.len();
    let (_, cleared) = row(ui, "Recent list", "", |ui| {
        ui.label(match count {
            0 => "empty".to_string(),
            1 => "1 project".to_string(),
            n => format!("{n} projects"),
        });
        ui.add_enabled(count > 0, egui::Button::new("Clear"))
            .on_hover_text("Forget the list. The project files stay where they are.")
            .clicked()
    });
    if cleared {
        app.settings.recent_projects.clear();
        app.session.status = match app.settings.try_save() {
            Ok(()) => "recent projects cleared".to_string(),
            Err(error) => format!("could not save the recent list: {error}"),
        };
    }

    #[cfg(windows)]
    file_association(app, ui);
}

#[cfg(windows)]
fn file_association(app: &mut EditorApp, ui: &mut Ui) {
    group(
        ui,
        "File association",
        "Which files open in q0editor. This affects the current Windows user only.",
    );
    let state = |registered: std::io::Result<bool>| match registered {
        Ok(true) => "active for this q0editor",
        Ok(false) => "not active",
        Err(_) => "status unavailable",
    };
    let q1s = state(crate::assoc::is_registered_for_current_user());
    row(ui, ".q1s project files", "", |ui| {
        ui.label(RichText::new(q1s).color(ui.visuals().weak_text_color()));
        if ui.button("Register .q1s").clicked() {
            app.session.status = match crate::assoc::register_for_current_user() {
                Ok(()) => ".q1s files now open with q0editor".to_string(),
                Err(error) => format!("association failed: {error}"),
            };
        }
        if ui.button("Remove .q1s association").clicked() {
            app.session.status = match crate::assoc::unregister_for_current_user() {
                Ok(()) => ".q1s association removed".to_string(),
                Err(error) => format!("removal failed: {error}"),
            };
        }
    });
    let q0lang = state(crate::assoc::is_q0lang_registered_for_current_user());
    row(
        ui,
        ".q0l / .q0lang source files",
        "Both extensions are the same q0lang source format.",
        |ui| {
            ui.label(RichText::new(q0lang).color(ui.visuals().weak_text_color()));
            if ui.button("Register .q0l + .q0lang").clicked() {
                app.session.status = match crate::assoc::register_q0lang_for_current_user() {
                    Ok(()) => ".q0l and .q0lang files now open with q0editor".to_string(),
                    Err(error) => format!("q0lang association failed: {error}"),
                };
            }
            if ui.button("Remove q0lang associations").clicked() {
                app.session.status = match crate::assoc::unregister_q0lang_for_current_user() {
                    Ok(()) => ".q0l and .q0lang associations removed".to_string(),
                    Err(error) => format!("removal failed: {error}"),
                };
            }
        },
    );
}

fn scripting(app: &mut EditorApp, ui: &mut Ui, page: &mut Page) {
    group(ui, "Editor", "");
    let (_, sized) = row(ui, "Font size", "", |ui| {
        let mut size = app.settings.q0lang_font_size.0;
        let response = ui.add(
            egui::DragValue::new(&mut size)
                .speed(0.5)
                .clamp_range(8.0..=72.0)
                .suffix(" pt"),
        );
        if response.changed() {
            app.settings.q0lang_font_size = PersistFontSize(size);
        }
        settled(&response)
    });
    let (_, tabbed) = row(ui, "Tab width", "Spaces a tab stands for", |ui| {
        let mut width = i32::from(app.settings.q0lang_tab_width);
        let response = ui.add(egui::DragValue::new(&mut width).clamp_range(1..=12));
        if response.changed() {
            app.settings.q0lang_tab_width = width.clamp(1, 12) as u8;
        }
        settled(&response)
    });
    let numbered = toggle_row(
        ui,
        "Line numbers",
        "",
        &mut app.settings.q0lang_show_line_numbers,
    );
    if sized || tabbed || numbered {
        app.settings.save();
    }
    see_also(
        ui,
        page,
        "Syntax colours are part of the theme.",
        Page::Appearance,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_is_found_by_its_name_and_by_a_keyword() {
        for page in Page::ALL {
            assert!(page.matches(&page.label().to_lowercase()));
            let word = page.keywords().split_whitespace().next().unwrap();
            assert!(page.matches(word), "{page:?} by {word}");
        }
        assert!(Page::General.matches("vsync"));
        assert!(Page::Shortcuts.matches("hotkeys"));
        assert!(!Page::Online.matches("vsync"));
    }

    #[test]
    fn settings_window_shows_each_page() {
        let ctx = Context::default();
        let mut app = EditorApp::default();
        crate::theme::install(&ctx);
        app.settings.theme.apply(&ctx);
        app.session.show_settings = true;
        for page in Page::ALL {
            ctx.data_mut(|d| d.insert_temp(Id::new(PAGE_ID), page));
            let mut output = None;
            for _ in 0..2 {
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            vec2(1280.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| render(&mut app, ctx),
                ));
            }
            let shown = output.unwrap().shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text)
                    if text.galley.job.text == page.detail())
            });
            assert!(shown, "{page:?} did not show");
            assert!(app.session.show_settings);
        }
    }
}
