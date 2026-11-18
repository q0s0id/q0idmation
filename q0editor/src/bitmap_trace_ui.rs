//! The Trace Bitmap dialog: the trace settings, the colours the bitmap will
//! be reduced to, and the button that traces it.

use std::sync::Arc;

use egui::{Context, Id, RichText};
use q0s_format::v2::{Asset, Rgba};

use crate::app::{Action, EditorApp};
use crate::bitmap_trace::{Bin, TraceSettings, MAX_COLORS};

#[derive(Clone)]
struct Dialog {
    asset_id: u16,
    settings: TraceSettings,
    /// The bitmap's colour buckets, worked out for `bins_for` (asset and
    /// opacity cutoff) so a slider drag does not go through every pixel.
    bins: Arc<Vec<Bin>>,
    bins_for: Option<(u16, u8)>,
    /// The colours shown, worked out for `palette_for` (asset, opacity
    /// cutoff and colour count).
    palette: Vec<Rgba>,
    palette_for: Option<(u16, u8, u8)>,
}

fn dialog_id() -> Id {
    Id::new("q0editor_trace_bitmap_dialog")
}

/// The settings last traced with, offered again next time.
fn last_settings_id() -> Id {
    Id::new("q0editor_trace_bitmap_last_settings")
}

fn selected_bitmap(app: &EditorApp) -> Option<u16> {
    app.selected_bitmap_placement()
        .map(|(.., asset_id)| asset_id)
}

pub(crate) fn open(app: &mut EditorApp, ctx: &Context) {
    let Some(asset_id) = selected_bitmap(app) else {
        app.session.status = "Trace Bitmap: select a bitmap on the stage first".to_string();
        return;
    };
    let settings = ctx
        .data(|data| data.get_temp::<TraceSettings>(last_settings_id()))
        .unwrap_or_default();
    ctx.data_mut(|data| {
        data.insert_temp(
            dialog_id(),
            Dialog {
                asset_id,
                settings,
                bins: Arc::default(),
                bins_for: None,
                palette: Vec::new(),
                palette_for: None,
            },
        );
    });
}

pub(crate) fn render(app: &mut EditorApp, ctx: &Context) {
    let Some(mut dialog) = ctx.data(|data| data.get_temp::<Dialog>(dialog_id())) else {
        return;
    };
    // The dialog follows the selection from one bitmap to the next.
    let selected = selected_bitmap(app);
    if let Some(asset_id) = selected {
        dialog.asset_id = asset_id;
    }
    let Some(bitmap) = app
        .state
        .project
        .assets
        .iter()
        .find_map(|asset| match asset {
            Asset::Bitmap(bitmap) if bitmap.asset_id == dialog.asset_id => Some(bitmap.clone()),
            _ => None,
        })
    else {
        ctx.data_mut(|data| data.remove::<Dialog>(dialog_id()));
        return;
    };
    let bins_for = (bitmap.asset_id, dialog.settings.alpha_threshold);
    if dialog.bins_for != Some(bins_for) {
        dialog.bins = crate::bitmap_trace::histogram(&bitmap.rgba, &dialog.settings);
        dialog.bins_for = Some(bins_for);
    }
    let palette_for = (bins_for.0, bins_for.1, dialog.settings.colors);
    if dialog.palette_for != Some(palette_for) {
        dialog.palette = crate::bitmap_trace::palette(&dialog.bins, dialog.settings.colors);
        dialog.palette_for = Some(palette_for);
    }

    let dim = app.settings.theme.text_dim.to_color32();
    let traceable = selected == Some(dialog.asset_id);
    let mut open = true;
    let mut confirm = false;
    let mut cancel = false;
    crate::dialog::Dialog::new("Trace Bitmap")
        .id(Id::new("q0editor_trace_bitmap_window"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .open(&mut open)
        .show(ctx, |ui| {
            let settings = &mut dialog.settings;
            egui::Grid::new("trace_bitmap_fields")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Colours");
                    ui.add(egui::Slider::new(&mut settings.colors, 2..=MAX_COLORS))
                        .on_hover_text(
                            "How many colours the bitmap is reduced to. Each colour becomes one shape.",
                        );
                    ui.end_row();

                    ui.label("Opacity cutoff");
                    ui.add(egui::Slider::new(&mut settings.alpha_threshold, 1..=255))
                        .on_hover_text("Pixels less opaque than this are left out.");
                    ui.end_row();

                    ui.label("Smallest patch");
                    ui.add(
                        egui::Slider::new(&mut settings.min_area, 1..=400)
                            .logarithmic(true)
                            .suffix(" px"),
                    )
                    .on_hover_text(
                        "Patches smaller than this join a colour next to them, which clears up noise and soft edges.",
                    );
                    ui.end_row();

                    ui.label("Curve fit");
                    ui.add(egui::Slider::new(&mut settings.curve_fit, 0.0..=4.0).suffix(" px"))
                        .on_hover_text(
                            "How far curves may stray from the pixel outline. 0 keeps the exact pixel edges, for pixel art.",
                        );
                    ui.end_row();

                    ui.label("Sharp corners");
                    ui.add(egui::Slider::new(&mut settings.corner_run, 0..=16).suffix(" px"))
                        .on_hover_text(
                            "Straight edges at least this long meet in a sharp corner instead of a rounded one. 0 rounds every corner.",
                        );
                    ui.end_row();
                });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                for color in &dialog.palette {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        2.0,
                        egui::Color32::from_rgb(color.r, color.g, color.b),
                    );
                    response.on_hover_text(format!(
                        "#{:02X}{:02X}{:02X}",
                        color.r, color.g, color.b
                    ));
                }
            });
            ui.label(
                RichText::new(format!(
                    "{} colours from a {}×{} bitmap. It stays in the library, and Undo brings it back.",
                    dialog.palette.len(),
                    bitmap.width,
                    bitmap.height
                ))
                .small()
                .color(dim),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(traceable, egui::Button::new("Trace"))
                    .on_disabled_hover_text("Select the bitmap on the stage to trace it")
                    .clicked()
                {
                    confirm = true;
                }
                if ui.button("Defaults").clicked() {
                    dialog.settings = TraceSettings::default();
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });

    let settings = dialog.settings;
    if open && !cancel && !confirm {
        ctx.data_mut(|data| data.insert_temp(dialog_id(), dialog));
        return;
    }
    ctx.data_mut(|data| data.remove::<Dialog>(dialog_id()));
    if confirm {
        ctx.data_mut(|data| data.insert_temp(last_settings_id(), settings));
        app.queue(Action::TraceBitmap(settings));
    }
}
