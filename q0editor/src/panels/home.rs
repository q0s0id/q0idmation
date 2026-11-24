//! The home page: a hero with a little scene playing beside it, sizes to
//! start a project from in one click, recent projects as cards with a cover
//! made from their name, and what is new in the latest releases.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, ScrollArea, Sense, Stroke, Ui};

use crate::app::{Action, EditorApp, NewProjectSpec};
use crate::release_feed::{
    parse_release_markdown, short_date, MarkdownBlock, MarkdownSpan, MarkdownSpanKind,
    ReleaseFeedStatus, ReleaseNote,
};

/// Stage sizes a project can start from in one click: name, width, height.
pub(crate) const QUICK_SIZES: [(&str, u16, u16); 5] = [
    ("Classic", 640, 480),
    ("HD", 1280, 720),
    ("Full HD", 1920, 1080),
    ("Portrait", 1080, 1920),
    ("Square", 1080, 1080),
];

pub fn render(app: &mut EditorApp, ui: &mut Ui) {
    ScrollArea::vertical()
        .id_source("home_page")
        .show(ui, |ui| {
            let page_width = (ui.available_width() - 48.0).clamp(300.0, 1100.0);
            let side_space = ((ui.available_width() - page_width) * 0.5).max(0.0);
            ui.add_space(28.0);
            ui.horizontal(|ui| {
                ui.add_space(side_space);
                ui.allocate_ui_with_layout(
                    vec2(page_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        hero(app, ui);
                        ui.add_space(18.0);
                        if !app.autosave.recovered().is_empty() {
                            recovered_work(app, ui);
                            ui.add_space(16.0);
                        }
                        quick_start(app, ui);
                        ui.add_space(22.0);
                        content_columns(app, ui);
                        ui.add_space(22.0);
                        footer(app, ui);
                        ui.add_space(24.0);
                    },
                );
            });
        });
}

fn dim(app: &EditorApp) -> Color32 {
    app.settings.theme.text_dim.to_color32()
}

fn accent(app: &EditorApp) -> Color32 {
    app.settings.theme.accent.to_color32()
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    crate::ui::blend(a, b, t)
}

/// The title, the two ways in, and a scene playing beside them.
fn hero(app: &mut EditorApp, ui: &mut Ui) {
    let accent = accent(app);
    let width = ui.available_width();
    let wide = width >= 760.0;
    let height = if wide { 214.0 } else { 260.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    let panel = app.settings.theme.panel.to_color32();
    let deep = app.settings.theme.deep_bg.to_color32();
    painter.rect_filled(rect, 16.0, mix(panel, deep, 0.35));
    // A still wash of the accent on the far side.
    for (grow, strength) in [(240.0, 0.05), (170.0, 0.06), (110.0, 0.07)] {
        painter.with_clip_rect(rect).circle_filled(
            pos2(rect.right() - 150.0, rect.center().y),
            grow,
            accent.gamma_multiply(strength),
        );
    }
    painter.rect_stroke(
        rect,
        16.0,
        Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );

    let text_width = width - 56.0;
    let left = rect.left() + 28.0;
    // The mark and the line under it.
    let mark = Rect::from_min_size(pos2(left, rect.top() + 26.0), vec2(30.0, 30.0));
    painter.rect_filled(mark, 8.0, accent);
    painter.text(
        mark.center(),
        Align2::CENTER_CENTER,
        "q0",
        FontId::proportional(14.0),
        crate::theme::on_accent(accent),
    );
    painter.text(
        pos2(mark.right() + 12.0, mark.center().y),
        Align2::LEFT_CENTER,
        "YOUR ANIMATION WORKSPACE",
        FontId::proportional(11.0),
        dim(app),
    );
    let title = painter.layout(
        "Make your next move.".to_string(),
        FontId::proportional(if wide { 38.0 } else { 30.0 }),
        ui.visuals().strong_text_color(),
        text_width,
    );
    let title_height = title.size().y;
    painter.galley(pos2(left, mark.bottom() + 18.0), title, Color32::WHITE);
    let line = painter.layout(
        "A new idea, a blank stage. Or pick up right where you left off.".to_string(),
        FontId::proportional(15.0),
        dim(app),
        text_width,
    );
    painter.galley(
        pos2(left, mark.bottom() + 26.0 + title_height),
        line,
        Color32::WHITE,
    );

    // The two ways in.
    let buttons_y = rect.bottom() - 62.0;
    let new = Rect::from_min_size(pos2(left, buttons_y), vec2(168.0, 40.0));
    let open = Rect::from_min_size(pos2(new.right() + 12.0, buttons_y), vec2(150.0, 40.0));
    if hero_button(ui, new, "New project", "Ctrl+N", accent, true).clicked() {
        app.queue(Action::NewProject);
    }
    if hero_button(ui, open, "Open…", "Ctrl+O", accent, false).clicked() {
        app.queue(Action::OpenProject);
    }
}

/// A big button of the hero: filled with the accent, or outlined, a shade
/// lighter under the pointer, with its shortcut.
fn hero_button(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    shortcut: &str,
    accent: Color32,
    primary: bool,
) -> egui::Response {
    let response = ui
        .interact(rect, ui.id().with(("home_hero", label)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id, response.hovered(), 0.14);
    let drawn = rect;
    let painter = ui.painter();
    painter.rect_filled(
        drawn.translate(vec2(0.0, 2.0)),
        10.0,
        Color32::from_black_alpha(40),
    );
    let (fill, text, stroke) = if primary {
        (
            mix(accent, Color32::WHITE, 0.12 * hover),
            crate::theme::on_accent(accent),
            Stroke::NONE,
        )
    } else {
        (
            mix(
                ui.visuals().widgets.inactive.weak_bg_fill,
                accent,
                0.12 * hover,
            ),
            ui.visuals().strong_text_color(),
            Stroke::new(
                1.0_f32,
                mix(
                    ui.visuals().widgets.noninteractive.bg_stroke.color,
                    accent,
                    hover,
                ),
            ),
        )
    };
    painter.rect(drawn, 10.0, fill, stroke);
    painter.text(
        drawn.left_center() + vec2(16.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        text,
    );
    painter.text(
        drawn.right_center() - vec2(12.0, 0.0),
        Align2::RIGHT_CENTER,
        shortcut,
        FontId::proportional(10.5),
        text.gamma_multiply(0.65),
    );
    response
}

/// Sizes to start from in one click, and one to choose everything.
fn quick_start(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Start from a size").size(15.0).strong());
        ui.label(
            RichText::new(format!(
                "{} fps, {} frames: change them under More options",
                app.settings.new_project_fps, app.settings.new_project_frame_count
            ))
            .small()
            .color(dim(app)),
        );
    });
    ui.add_space(8.0);
    let gap = 10.0;
    let count = QUICK_SIZES.len() + 1;
    let width = ui.available_width();
    let columns = if width >= 700.0 { count } else { 3 };
    let card_width = (width - gap * (columns - 1) as f32) / columns as f32;
    let rows = count.div_ceil(columns);
    let (grid, _) = ui.allocate_exact_size(
        vec2(width, rows as f32 * 78.0 + (rows - 1) as f32 * gap),
        Sense::hover(),
    );
    let accent = accent(app);
    for index in 0..count {
        let rect = Rect::from_min_size(
            grid.min
                + vec2(
                    (index % columns) as f32 * (card_width + gap),
                    (index / columns) as f32 * (78.0 + gap),
                ),
            vec2(card_width, 78.0),
        );
        let custom = index == QUICK_SIZES.len();
        let (name, w, h) = QUICK_SIZES
            .get(index)
            .copied()
            .unwrap_or(("More options…", 0, 0));
        let response = ui
            .interact(rect, ui.id().with(("home_quick", index)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if custom {
                "Name, size, frame rate, length and background".to_string()
            } else {
                format!("A new {w} × {h} project, right away")
            });
        let hover = ui
            .ctx()
            .animate_bool_with_time(response.id, response.hovered(), 0.14);
        let drawn = rect;
        let painter = ui.painter();
        painter.rect_filled(
            drawn.translate(vec2(0.0, 2.0)),
            12.0,
            Color32::from_black_alpha(25),
        );
        painter.rect(
            drawn,
            12.0,
            mix(
                ui.visuals().widgets.inactive.weak_bg_fill,
                accent,
                0.08 * hover,
            ),
            Stroke::new(
                1.0_f32,
                mix(
                    ui.visuals().widgets.noninteractive.bg_stroke.color,
                    accent,
                    hover,
                ),
            ),
        );
        // The stage's shape.
        let glyph_box = Rect::from_center_size(
            pos2(drawn.left() + 34.0, drawn.center().y),
            vec2(40.0, 40.0),
        );
        if custom {
            let c = glyph_box.center();
            for (from, to) in [
                (vec2(-9.0, 0.0), vec2(9.0, 0.0)),
                (vec2(0.0, -9.0), vec2(0.0, 9.0)),
            ] {
                painter.line_segment([c + from, c + to], Stroke::new(2.0_f32, accent));
            }
            painter.circle_stroke(c, 15.0, Stroke::new(1.2_f32, accent.gamma_multiply(0.6)));
        } else {
            let aspect = f32::from(w) / f32::from(h.max(1));
            let size = if aspect >= 1.0 {
                vec2(34.0, 34.0 / aspect)
            } else {
                vec2(34.0 * aspect, 34.0)
            };
            let glyph = Rect::from_center_size(glyph_box.center(), size);
            painter.rect(
                glyph,
                3.0,
                accent.gamma_multiply(0.18 + 0.12 * hover),
                Stroke::new(1.5_f32, accent),
            );
        }
        let text_left = glyph_box.right() + 8.0;
        painter.text(
            pos2(text_left, drawn.center().y - 9.0),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(14.0),
            ui.visuals().strong_text_color(),
        );
        painter.text(
            pos2(text_left, drawn.center().y + 10.0),
            Align2::LEFT_CENTER,
            if custom {
                "Choose everything".to_string()
            } else {
                format!("{w} × {h}")
            },
            FontId::proportional(11.0),
            dim(app),
        );
        if response.clicked() {
            if custom {
                app.queue(Action::NewProject);
            } else {
                app.queue(Action::CreateProject(NewProjectSpec {
                    width: w,
                    height: h,
                    fps: app.settings.new_project_fps,
                    frame_count: app.settings.new_project_frame_count,
                    ..NewProjectSpec::default()
                }));
            }
        }
    }
}

fn content_columns(app: &mut EditorApp, ui: &mut Ui) {
    app.release_feed.ensure_started(ui.ctx());
    app.release_feed.poll();
    let columns = if ui.available_width() >= 760.0 { 2 } else { 1 };
    if columns == 2 {
        ui.columns(2, |columns| {
            crate::theme::surface(&columns[0]).show(&mut columns[0], |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(300.0);
                recent_projects(app, ui);
            });
            crate::theme::surface(&columns[1]).show(&mut columns[1], |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(300.0);
                release_feed(app, ui);
            });
        });
    } else {
        crate::theme::surface(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            recent_projects(app, ui);
        });
        ui.add_space(16.0);
        crate::theme::surface(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            release_feed(app, ui);
        });
    }
}

/// When each recent file last changed: `None` while it is still being
/// looked up, `Some(None)` for a file that is gone, `Some(Some(time))` (or
/// `Some(Some(None))` with no time) otherwise. Looked up on a thread of its
/// own and again after a minute: a project on a slow or unplugged drive
/// stalled the whole page while it was asked.
fn modified(ctx: &egui::Context, path: &Path) -> Option<Option<Option<SystemTime>>> {
    type Found = Option<Option<SystemTime>>;
    static SEEN: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, (Instant, Found)>>,
    > = std::sync::OnceLock::new();
    static ASKING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    let asking = ASKING.get_or_init(Default::default);
    let known = seen.lock().ok().and_then(|seen| seen.get(path).copied());
    let stale = known.is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(60));
    if stale {
        let fresh = asking
            .lock()
            .map(|mut asking| asking.insert(path.to_path_buf()))
            .unwrap_or(false);
        if fresh {
            let path = path.to_path_buf();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let found = std::fs::metadata(&path)
                    .ok()
                    .map(|metadata| metadata.modified().ok());
                if let Ok(mut seen) = seen.lock() {
                    seen.insert(path.clone(), (Instant::now(), found));
                }
                if let Ok(mut asking) = asking.lock() {
                    asking.remove(&path);
                }
                ctx.request_repaint();
            });
        }
    }
    known.map(|(_, found)| found)
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "12 days ago".
fn ago(when: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(when)
        .map_or(0, |since| since.as_secs());
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        86_400..=172_799 => "yesterday".to_string(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

fn recent_projects(app: &mut EditorApp, ui: &mut Ui) {
    let recent = app.settings.recent_projects.clone();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Recent projects").size(17.0).strong());
        ui.label(
            RichText::new(format!("{:02}", recent.len()))
                .small()
                .color(dim(app)),
        );
    });
    ui.add_space(10.0);
    if recent.is_empty() {
        ui.add_space(20.0);
        ui.label(
            RichText::new("Your next story starts here.")
                .size(16.0)
                .strong(),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Projects you open or save will appear here, ready for your next session.",
            )
            .color(dim(app)),
        );
        ui.add_space(16.0);
        if crate::theme::primary_button(ui, "Create your first project", accent(app)).clicked() {
            app.queue(Action::NewProject);
        }
        return;
    }
    let search_id = ui.id().with("home_recent_search");
    let mut search: String = ui.data(|data| data.get_temp(search_id)).unwrap_or_default();
    if recent.len() > 5 {
        crate::ui::search_field(ui, &mut search, "Find a project...");
        ui.data_mut(|data| data.insert_temp(search_id, search.clone()));
        ui.add_space(6.0);
    }
    let needle = search.trim().to_lowercase();
    let mut open = None;
    let mut forget = None;
    ScrollArea::vertical()
        .id_source("home_recent_projects")
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (index, path) in recent.iter().enumerate().filter(|(_, path)| {
                needle.is_empty() || path.to_string_lossy().to_lowercase().contains(&needle)
            }) {
                match recent_card(app, ui, path, index) {
                    Some(RecentChoice::Open) => open = Some(path.clone()),
                    Some(RecentChoice::Forget) => forget = Some(path.clone()),
                    None => {}
                }
                ui.add_space(6.0);
            }
        });
    if let Some(path) = open {
        app.queue(Action::OpenProjectFromPath(path));
    }
    if let Some(path) = forget {
        app.settings
            .recent_projects
            .retain(|recent| *recent != path);
        app.settings.save();
    }
}

enum RecentChoice {
    Open,
    Forget,
}

/// A recent project as a card: a cover made from its name, the name, its
/// folder and when it last changed. A file that is gone says so and can be
/// taken off the list.
fn recent_card(app: &EditorApp, ui: &mut Ui, path: &Path, index: usize) -> Option<RecentChoice> {
    let file_name = path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let folder = path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let state = modified(ui.ctx(), path);
    let missing = matches!(state, Some(None));
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 62.0), Sense::click());
    let response = response
        .on_hover_text(path.display().to_string())
        .on_hover_cursor(if missing {
            egui::CursorIcon::Default
        } else {
            egui::CursorIcon::PointingHand
        });
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, path.display().to_string())
    });
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id, response.hovered(), 0.14);
    let accent = accent(app);
    let painter = ui.painter_at(rect.expand(4.0));
    let drawn = rect;
    painter.rect(
        drawn,
        10.0,
        mix(
            ui.visuals().widgets.inactive.weak_bg_fill,
            accent,
            0.07 * hover,
        ),
        Stroke::new(
            1.0_f32,
            mix(
                ui.visuals().widgets.noninteractive.bg_stroke.color,
                accent,
                0.7 * hover,
            ),
        ),
    );
    let left = drawn.left() + 16.0;
    let strong = ui.visuals().strong_text_color();
    let name_color = if missing { dim(app) } else { strong };
    let right_room = 96.0;
    crate::ui::clipped_label(
        &painter,
        Rect::from_min_max(
            pos2(left, drawn.top() + 10.0),
            pos2(drawn.right() - right_room, drawn.top() + 30.0),
        ),
        &file_name,
        FontId::proportional(14.5),
        name_color,
    );
    crate::ui::clipped_label(
        &painter,
        Rect::from_min_max(
            pos2(left, drawn.top() + 33.0),
            pos2(drawn.right() - right_room, drawn.bottom() - 10.0),
        ),
        &folder,
        FontId::proportional(11.0),
        dim(app),
    );
    let mut choice = None;
    if missing {
        let pill = Rect::from_min_size(
            pos2(drawn.right() - 88.0, drawn.top() + 10.0),
            vec2(56.0, 18.0),
        );
        painter.rect_filled(pill, 9.0, ui.visuals().error_fg_color.gamma_multiply(0.2));
        painter.text(
            pill.center(),
            Align2::CENTER_CENTER,
            "missing",
            FontId::proportional(10.5),
            ui.visuals().error_fg_color,
        );
    } else if let Some(Some(Some(when))) = state {
        painter.text(
            pos2(drawn.right() - 32.0, drawn.top() + 19.0),
            Align2::RIGHT_CENTER,
            ago(when),
            FontId::proportional(10.5),
            dim(app),
        );
    }
    // Under the pointer: a cross to take it off the list, an arrow to open.
    if hover > 0.01 || missing {
        let cross = Rect::from_center_size(
            pos2(drawn.right() - 16.0, drawn.top() + 19.0),
            vec2(18.0, 18.0),
        );
        let forget = ui
            .interact(
                cross,
                ui.id().with(("home_recent_forget", index)),
                Sense::click(),
            )
            .on_hover_text("Take it off this list (the file stays)");
        let lit = if forget.hovered() { 1.0 } else { 0.55 };
        let c = cross.center();
        let ink = dim(app).gamma_multiply(lit * hover.max(if missing { 1.0 } else { 0.0 }));
        painter.line_segment(
            [c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)],
            Stroke::new(1.4_f32, ink),
        );
        painter.line_segment(
            [c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)],
            Stroke::new(1.4_f32, ink),
        );
        if forget.clicked() {
            choice = Some(RecentChoice::Forget);
        }
        if !missing {
            let tip = pos2(drawn.right() - 14.0, drawn.bottom() - 17.0);
            painter.line_segment(
                [tip + vec2(-5.0, -5.0), tip],
                Stroke::new(1.6_f32, accent.gamma_multiply(hover)),
            );
            painter.line_segment(
                [tip + vec2(-5.0, 5.0), tip],
                Stroke::new(1.6_f32, accent.gamma_multiply(hover)),
            );
        }
    }
    if choice.is_none() && response.clicked() && !missing {
        choice = Some(RecentChoice::Open);
    }
    choice
}

fn release_feed(app: &mut EditorApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("What’s new").size(17.0).strong());
        let pill = RichText::new(" GitHub releases ")
            .small()
            .color(accent(app))
            .background_color(accent(app).gamma_multiply(0.15));
        ui.label(pill);
    });
    ui.add_space(10.0);

    let status = app.release_feed.status().clone();
    match status {
        ReleaseFeedStatus::Idle | ReleaseFeedStatus::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading releases from GitHub...").color(dim(app)));
            });
        }
        ReleaseFeedStatus::Ready(releases) if releases.is_empty() => {
            ui.label(RichText::new("No published GitHub releases yet.").color(dim(app)));
        }
        ReleaseFeedStatus::Ready(releases) => {
            ScrollArea::vertical()
                .id_source("home_release_feed")
                .auto_shrink([false, false])
                .max_height(320.0)
                .show(ui, |ui| {
                    for (index, release) in releases.iter().enumerate() {
                        if index > 0 {
                            ui.add_space(14.0);
                        }
                        release_card(app, ui, release, index == 0);
                    }
                });
        }
        ReleaseFeedStatus::Error(error) => {
            ui.label(RichText::new("GitHub feed is temporarily unavailable.").strong());
            ui.add_space(4.0);
            ui.label(RichText::new(error).small().color(dim(app)));
            ui.add_space(10.0);
            if ui.button("Retry").clicked() {
                app.release_feed.retry(ui.ctx());
            }
        }
    }
}

/// One release: its version in a pill, the newest marked as such, its
/// notes, and a link to it, along a line that joins the releases up.
fn release_card(app: &EditorApp, ui: &mut Ui, release: &ReleaseNote, newest: bool) {
    let accent = accent(app);
    let top = ui.cursor().top();
    let rail = ui.cursor().left() + 5.0;
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(" {} ", release.tag))
                        .small()
                        .strong()
                        .color(crate::theme::on_accent(accent))
                        .background_color(accent),
                );
                if newest {
                    ui.label(RichText::new("latest").small().color(accent));
                }
                if release.prerelease {
                    ui.label(RichText::new("prerelease").small().color(dim(app)));
                }
                if let Some(date) = short_date(release.published_at.as_deref()) {
                    ui.label(RichText::new(date).small().color(dim(app)));
                }
            });
            ui.add_space(2.0);
            ui.label(RichText::new(&release.title).size(15.0).strong());
            ui.add_space(4.0);
            render_release_markdown(app, ui, &release.body);
            ui.add_space(6.0);
            ui.hyperlink_to("View release on GitHub", &release.url);
        });
    });
    let bottom = ui.min_rect().bottom();
    let painter = ui.painter();
    painter.line_segment(
        [pos2(rail, top + 8.0), pos2(rail, bottom)],
        Stroke::new(1.0_f32, dim(app).gamma_multiply(0.3)),
    );
    painter.circle_filled(
        pos2(rail, top + 8.0),
        if newest { 4.5 } else { 3.5 },
        accent,
    );
    if newest {
        painter.circle_stroke(
            pos2(rail, top + 8.0),
            7.5,
            Stroke::new(1.0_f32, accent.gamma_multiply(0.4)),
        );
    }
}

fn footer(app: &EditorApp, ui: &mut Ui) {
    const TIPS: [&str; 8] = [
        "Hold Alt while drawing a rectangle or oval to grow it from its middle.",
        "F6 makes a keyframe, F7 a blank one, F5 adds frames.",
        "Shift snaps turning and lines to steps you can set under Drawing.",
        "Edit frames lets you move a whole stretch of frames at once.",
        "Drag a layer's square in the parenting view onto another layer to hang it there.",
        "Right-click a span of frames to make a motion or shape tween.",
        "The pencil's Straighten mode turns wobbly lines into clean ones.",
        "Ctrl+wheel over the timeline zooms its frames.",
    ];
    let day = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400) as usize;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("q0editor").strong());
        ui.label(
            RichText::new(env!("CARGO_PKG_VERSION"))
                .small()
                .color(dim(app)),
        );
        ui.separator();
        ui.label(RichText::new("Tip").small().strong().color(accent(app)));
        ui.label(
            RichText::new(TIPS[day % TIPS.len()])
                .small()
                .color(dim(app)),
        );
    });
}

/// Snapshots left behind by a session that did not exit cleanly.
fn recovered_work(app: &mut EditorApp, ui: &mut Ui) {
    use crate::autosave::DiscardTarget;

    let dim = app.settings.theme.text_dim.to_color32();
    let accent = app.settings.theme.accent.to_color32();
    let rows = app
        .autosave
        .recovered()
        .iter()
        .map(|entry| {
            (
                entry.title().to_string(),
                entry.detail(),
                entry.original_path(),
                entry.key().to_path_buf(),
            )
        })
        .collect::<Vec<_>>();
    let armed = app.autosave.discard_armed.clone();
    let mut open = None;
    let mut discard = None;
    let mut discard_all = false;
    let mut arm = None;
    crate::theme::surface(ui)
        .fill(app.settings.theme.panel.to_color32())
        .stroke(egui::Stroke::new(1.0_f32, accent))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Recovered work").size(19.0).strong());
            ui.label(
                RichText::new(
                    "q0editor did not close cleanly. These unsaved documents were kept; \
                     opening one changes nothing on disk until you save.",
                )
                .color(dim),
            );
            ui.add_space(12.0);
            for (index, (title, detail, path, key)) in rows.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    if crate::theme::primary_button(ui, "Open", accent).clicked() {
                        open = Some(index);
                    }
                    let this = DiscardTarget::One(key.clone());
                    if armed.as_ref() == Some(&this) {
                        if ui.button("Delete for good").clicked() {
                            discard = Some(index);
                        }
                    } else if ui.button("Discard").clicked() {
                        arm = Some(this);
                    }
                    ui.label(RichText::new(title).strong());
                    ui.label(RichText::new(detail).small().color(dim));
                    let origin = match path {
                        Some(path) => path.display().to_string(),
                        None => "never saved".to_string(),
                    };
                    ui.label(RichText::new(origin).small().color(dim));
                });
                ui.add_space(4.0);
            }
            if rows.len() > 1 {
                ui.add_space(4.0);
                if armed == Some(DiscardTarget::All) {
                    if ui.button("Delete all for good").clicked() {
                        discard_all = true;
                    }
                } else if ui.button("Discard all").clicked() {
                    arm = Some(DiscardTarget::All);
                }
            }
        });
    if let Some(target) = arm {
        app.autosave.discard_armed = Some(target);
    }
    if let Some(index) = open {
        app.open_recovered_document(index);
    } else if let Some(index) = discard {
        app.autosave.discard_recovered(index);
    } else if discard_all {
        app.autosave.discard_all_recovered();
    }
}

fn render_release_markdown(app: &EditorApp, ui: &mut Ui, body: &str) {
    for block in parse_release_markdown(body) {
        match block {
            MarkdownBlock::Heading { level, spans } => {
                if level <= 2 {
                    ui.add_space(5.0);
                }
                let size = match level {
                    1 => 16.0,
                    2 => 14.5,
                    _ => 13.5,
                };
                render_markdown_spans(app, ui, &spans, size, true, false);
                ui.add_space(2.0);
            }
            MarkdownBlock::Bullet(spans) => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("• ").color(ui.visuals().hyperlink_color));
                    render_markdown_spans_inline(app, ui, &spans, 13.0, false, true);
                });
                ui.add_space(2.0);
            }
            MarkdownBlock::Paragraph(spans) => {
                render_markdown_spans(app, ui, &spans, 13.0, false, true);
                ui.add_space(2.0);
            }
            MarkdownBlock::Spacer => ui.add_space(5.0),
        }
    }
}

fn render_markdown_spans(
    app: &EditorApp,
    ui: &mut Ui,
    spans: &[MarkdownSpan],
    size: f32,
    strong_all: bool,
    dim_plain: bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        render_markdown_spans_inline(app, ui, spans, size, strong_all, dim_plain);
    });
}

fn render_markdown_spans_inline(
    app: &EditorApp,
    ui: &mut Ui,
    spans: &[MarkdownSpan],
    size: f32,
    strong_all: bool,
    dim_plain: bool,
) {
    let accent = ui.visuals().hyperlink_color;
    let normal = if dim_plain {
        app.settings.theme.text_dim.to_color32()
    } else {
        ui.visuals().text_color()
    };

    for span in spans {
        let mut text = RichText::new(&span.text).size(size).color(normal);
        if strong_all || matches!(span.kind, MarkdownSpanKind::Strong) {
            text = text.strong();
        }
        match &span.kind {
            MarkdownSpanKind::Code => {
                ui.label(text.monospace().color(accent));
            }
            MarkdownSpanKind::Emphasis => {
                ui.label(text.italics());
            }
            MarkdownSpanKind::Link(url) => {
                ui.hyperlink_to(text.color(accent).underline(), url);
            }
            MarkdownSpanKind::Plain | MarkdownSpanKind::Strong => {
                ui.label(text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_work_lists_every_snapshot_of_a_crashed_session() {
        let root = crate::autosave::test_support::TestDir::new("home");
        let ctx = egui::Context::default();
        crate::autosave::test_support::leave_crashed_script(&root, "go 4");

        let mut app = EditorApp::default();
        crate::autosave::test_support::enable(&mut app, &root);
        crate::autosave::tick(&mut app, &ctx);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| recovered_work(&mut app, ui));
        });
        assert!(!output.shapes.is_empty());
        assert_eq!(
            app.autosave.recovered().len(),
            1,
            "drawing discards nothing"
        );
        assert_eq!(root.files().len(), 2);
    }
}
