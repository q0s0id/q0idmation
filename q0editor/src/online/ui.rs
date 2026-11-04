//! the online panel: sign-in, rooms, friends, participants and permissions.
//! it reads controller state and queues commands; it never touches the network.
use super::*;
use egui::{vec2, Align, Color32, Layout, RichText, Stroke, Ui};
use q0online::{layer_access, library_access, project_access, Access, Grant, Scope};
use transport::FileTarget;

/// Right-click a line or message to report it to the moderators.
fn report_menu(response: &egui::Response, out: &mut Out, kind: &'static str, id: i64) {
    response.context_menu(|ui| {
        ui.label(RichText::new("Report to the moderators").small().strong());
        for reason in chat::REPORT_REASONS {
            if ui.button(reason).clicked() {
                out.send.push(Command::Report {
                    kind,
                    target_id: id.to_string(),
                    reason,
                });
                ui.close_menu();
            }
        }
    });
}
/// A picture or video to send; pictures and videos only, as the server takes.
fn pick_media() -> Option<std::path::PathBuf> {
    crate::platform::FileDialog::new()
        .add_filter(
            "Pictures and videos",
            &["png", "jpg", "jpeg", "gif", "webp", "mp4", "webm"],
        )
        .pick_file()
}

const ARM_FOR: Duration = Duration::from_millis(4500);
const ROW_AVATAR: f32 = 26.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Tab {
    Room,
    Chat,
    #[default]
    Rooms,
    Friends,
    Messages,
}
struct MemberDraft {
    user_id: i64,
    rank: Rank,
    grants: Vec<Grant>,
    base: Member,
}
pub(super) struct View {
    pub(super) tab: Tab,
    room_name: String,
    room_name_edited: bool,
    join_code: String,
    pub(super) search: String,
    pub(super) expanded: Option<i64>,
    member_draft: Option<MemberDraft>,
    policy_draft: Option<RoomPolicy>,
    policy_base: Option<RoomPolicy>,
    invite_rank: Rank,
    filter: String,
    defaults_filter: [String; 2],
    armed: Option<(String, Instant)>,
    /// set when a joined room could not open yet; the panel offers a retry.
    pub(super) hold_join: bool,
    pub(super) recovery_hidden: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            room_name: String::new(),
            room_name_edited: false,
            join_code: String::new(),
            search: String::new(),
            expanded: None,
            member_draft: None,
            policy_draft: None,
            policy_base: None,
            // people are usually invited to draw; viewers are the exception
            invite_rank: Rank::Editor,
            filter: String::new(),
            defaults_filter: Default::default(),
            armed: None,
            hold_join: false,
            recovery_hidden: false,
        }
    }
}
impl View {
    pub(super) fn joined_room(&mut self) {
        self.tab = Tab::Room;
        self.forget_room();
        self.hold_join = false;
    }
    pub(super) fn left_room(&mut self) {
        if matches!(self.tab, Tab::Room | Tab::Chat) {
            self.tab = Tab::Rooms;
        }
        self.forget_room();
    }
    fn forget_room(&mut self) {
        self.expanded = None;
        self.member_draft = None;
        self.policy_draft = None;
        self.policy_base = None;
        self.filter.clear();
        self.armed = None;
    }
    fn is_armed(&self, key: &str) -> bool {
        self.armed
            .as_ref()
            .is_some_and(|(armed, at)| armed == key && at.elapsed() < ARM_FOR)
    }
}

/// Colours come from the active editor theme, never from a separate palette.
struct Palette {
    accent: Color32,
    on_accent: Color32,
    link: Color32,
    text: Color32,
    dim: Color32,
    warn: Color32,
    error: Color32,
    /// Live, online and done. Themes have no success colour, so this one
    /// status green follows only light or dark mode.
    ok: Color32,
    card: Color32,
    line: Color32,
}
impl Palette {
    fn new(app: &EditorApp, ui: &Ui) -> Self {
        let visuals = ui.visuals();
        let accent = app.settings.theme.accent.to_color32();
        Self {
            accent,
            on_accent: crate::theme::on_accent(accent),
            link: visuals.hyperlink_color,
            text: visuals.text_color(),
            dim: visuals.weak_text_color(),
            warn: visuals.warn_fg_color,
            error: visuals.error_fg_color,
            ok: if visuals.dark_mode {
                Color32::from_rgb(0x5C, 0xC2, 0x7A)
            } else {
                Color32::from_rgb(0x2E, 0x7D, 0x46)
            },
            card: visuals.faint_bg_color,
            line: visuals.widgets.noninteractive.bg_stroke.color,
        }
    }
    fn tone(&self, tone: Tone) -> Color32 {
        match tone {
            Tone::Info => self.dim,
            Tone::Success => self.ok,
            Tone::Warning => self.warn,
            Tone::Error => self.error,
        }
    }
}

/// Things the panel decided this frame, applied after drawing.
#[derive(Default)]
struct Out {
    send: Vec<Command>,
    say: Option<(String, Tone)>,
    avatars: Vec<i64>,
    sign_in: bool,
    cancel_sign_in: bool,
    leave: bool,
    logout: bool,
    open_draft: bool,
    open_join: bool,
    follow: Option<Follow>,
    history: Option<bool>,
    /// Open the conversation with this person in the Messages tab.
    message: Option<i64>,
}
impl Out {
    fn say(&mut self, text: impl Into<String>, tone: Tone) {
        self.say = Some((text.into(), tone));
    }
}

// --- small widgets -----------------------------------------------------------

fn card<R>(ui: &mut Ui, pal: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(pal.card)
        .stroke(Stroke::new(1.0_f32, pal.line))
        .rounding(6.0)
        .inner_margin(egui::Margin::symmetric(10.0, 9.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}
/// A single row laid out from the right: buttons first, then a field that
/// fills what is left. Sized to one control height so it can't take the
/// rest of the panel, and it never asks for more width than it has.
fn one_line<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), add)
        .inner
}
fn heading(ui: &mut Ui, pal: &Palette, title: &str, detail: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).strong().size(14.0).color(pal.text));
    if !detail.is_empty() {
        ui.label(RichText::new(detail).small().color(pal.dim));
    }
    ui.add_space(2.0);
}
fn primary(ui: &mut Ui, pal: &Palette, label: &str, enabled: bool) -> egui::Response {
    let fill = if enabled { pal.accent } else { pal.card };
    let text = if enabled { pal.on_accent } else { pal.dim };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).strong().color(text))
            .fill(fill)
            .min_size(vec2(0.0, 28.0)),
    )
}
fn chip(ui: &mut Ui, text: &str, color: Color32, filled: bool) -> egui::Response {
    let text_color = if filled {
        crate::theme::on_accent(color)
    } else {
        color
    };
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, text_color);
    let size = galley.size() + vec2(12.0, 3.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let fill = if filled { color } else { Color32::TRANSPARENT };
        ui.painter()
            .rect(rect, size.y * 0.5, fill, Stroke::new(1.0_f32, color));
        ui.painter()
            .galley(rect.center() - galley.size() * 0.5, galley, text_color);
    }
    response
}
fn rank_text(rank: Rank) -> &'static str {
    match rank {
        Rank::Viewer => "viewer",
        Rank::Editor => "editor",
        Rank::Admin => "admin",
        Rank::Owner => "owner",
    }
}
fn rank_help(rank: Rank) -> &'static str {
    match rank {
        Rank::Viewer => "Watches the live project. Can't edit.",
        Rank::Editor => "Edits what the owner assigns, plus editor defaults.",
        Rank::Admin => "Moderates lower ranks. Editing is assigned separately.",
        Rank::Owner => "Controls ranks, assignments and room settings.",
    }
}
fn rank_chip(ui: &mut Ui, pal: &Palette, rank: Rank) -> egui::Response {
    let color = match rank {
        Rank::Owner | Rank::Admin => pal.accent,
        Rank::Editor => pal.link,
        Rank::Viewer => pal.dim,
    };
    chip(ui, rank_text(rank), color, rank == Rank::Owner).on_hover_text(rank_help(rank))
}
fn dot(ui: &mut Ui, color: Color32, filled: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
    if filled {
        ui.painter().circle_filled(rect.center(), 4.0, color);
    } else {
        ui.painter()
            .circle_stroke(rect.center(), 3.5, Stroke::new(1.0_f32, color));
    }
    response
}
/// Avatar image, or initials on a colour derived from the account id.
fn avatar(
    ui: &mut Ui,
    avatars: &HashMap<i64, Avatar>,
    person: &Person,
    size: f32,
    out: &mut Out,
) -> egui::Response {
    match avatars.get(&person.id) {
        Some(Avatar::Ready(texture)) => {
            return ui.add(
                egui::Image::new(egui::load::SizedTexture::new(
                    texture.id(),
                    vec2(size, size),
                ))
                .rounding(size * 0.5),
            );
        }
        None if person.id > 0 => out.avatars.push(person.id),
        _ => {}
    }
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    let hue = ((person.id.unsigned_abs().wrapping_mul(2654435761) % 360) as f32) / 360.0;
    let dark = ui.visuals().dark_mode;
    let fill: Color32 =
        egui::ecolor::Hsva::new(hue, 0.45, if dark { 0.42 } else { 0.78 }, 1.0).into();
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    let initial = person
        .username
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.48),
        crate::theme::on_accent(fill),
    );
    response
}
fn ago(now: i64, t: i64) -> String {
    let s = (now - t).max(0);
    match s {
        0..=89 => "just now".into(),
        90..=3599 => format!("{} min ago", (s + 30) / 60),
        3600..=86_399 => format!("{} h ago", (s + 1800) / 3600),
        _ => format!("{} d ago", (s + 43_200) / 86_400),
    }
}
fn until(now: i64, t: i64) -> String {
    let s = t - now;
    match s {
        i64::MIN..=0 => "expired".into(),
        1..=3599 => format!("expires in {} min", (s + 59) / 60),
        3600..=86_399 => format!("expires in {} h", s / 3600),
        _ => format!("expires in {} d", s / 86_400),
    }
}
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
fn copy(ui: &Ui, text: &str, out: &mut Out, what: &str) {
    ui.ctx().copy_text(text.to_owned());
    out.say(format!("{what} copied"), Tone::Success);
}
/// Two clicks for anything that is hard to take back; the first click arms it.
fn armed_button(
    ui: &mut Ui,
    view: &mut View,
    pal: &Palette,
    key: &str,
    label: &str,
    confirm: &str,
    enabled: bool,
) -> bool {
    let armed = view.is_armed(key);
    let button = if armed {
        egui::Button::new(
            RichText::new(confirm)
                .strong()
                .color(crate::theme::on_accent(pal.error)),
        )
        .fill(pal.error)
    } else {
        egui::Button::new(RichText::new(label).color(pal.error))
    };
    let response = ui.add_enabled(enabled, button);
    if armed {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    if response.clicked() {
        if armed {
            view.armed = None;
            return true;
        }
        view.armed = Some((key.to_owned(), Instant::now()));
    }
    false
}

// --- names for project resources ---------------------------------------------

fn q0rg_name(project: &ProjectV2, q0rg_id: u16) -> String {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .map(|q| q.name.clone())
        .unwrap_or_else(|| format!("symbol {q0rg_id}"))
}
fn layer_name(project: &ProjectV2, q0rg_id: u16, layer_id: u16) -> Option<String> {
    project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)?
        .layers
        .iter()
        .find(|l| l.layer_id == layer_id)
        .map(|l| l.name.clone())
}
fn scope_label(project: &ProjectV2, scope: &Scope) -> String {
    let context = |q0rg_id: u16| {
        if project.q0rgs.len() > 1 {
            format!(" · {}", q0rg_name(project, q0rg_id))
        } else {
            String::new()
        }
    };
    match scope {
        Scope::Project => "whole project".into(),
        Scope::Layer { q0rg_id, layer_id } => format!(
            "{}{}",
            layer_name(project, *q0rg_id, *layer_id).unwrap_or_else(|| format!("layer {layer_id}")),
            context(*q0rg_id)
        ),
        Scope::LayerFolder { q0rg_id, layer_id } => format!(
            "{}/{}",
            layer_name(project, *q0rg_id, *layer_id)
                .unwrap_or_else(|| format!("folder {layer_id}")),
            context(*q0rg_id)
        ),
        Scope::LibraryFolder { path } if path.is_empty() => "entire library".into(),
        Scope::LibraryFolder { path } => format!("library {path}/"),
    }
}
fn scope_hint(scope: &Scope) -> &'static str {
    match scope {
        Scope::Project => "Every layer, symbol, library item and project setting.",
        Scope::Layer { .. } => "This timeline layer.",
        Scope::LayerFolder { .. } => "This timeline folder and everything inside it.",
        Scope::LibraryFolder { .. } => {
            "Symbols and shared artwork in this library folder and below, including their instances."
        }
    }
}
/// Rows for one timeline, top layer first like the timeline panel.
fn timeline_rows(
    project: &ProjectV2,
    q0rg: &q0s_format::v2::Q0rg,
) -> Vec<(u16, String, bool, usize)> {
    q0rg.layers
        .iter()
        .rev()
        .map(|layer| {
            (
                layer.layer_id,
                layer.name.clone(),
                project.layer_is_folder(q0rg.q0rg_id, layer.layer_id),
                project.layer_folder_depth(q0rg.q0rg_id, layer.layer_id),
            )
        })
        .collect()
}
struct Reach {
    whole: bool,
    editable: usize,
    total: usize,
    library: usize,
}
fn reach(project: &ProjectV2, policy: &RoomPolicy, member: &Member) -> Reach {
    let mut editable = 0;
    let mut total = 0;
    for q0rg in &project.q0rgs {
        for layer in &q0rg.layers {
            total += 1;
            if layer_access(project, policy, member, q0rg.q0rg_id, layer.layer_id).allowed() {
                editable += 1;
            }
        }
    }
    let library = policy
        .rank_defaults(member.rank)
        .iter()
        .chain(&member.grants)
        .filter(|g| matches!(g.scope, Scope::LibraryFolder { .. }))
        .count();
    Reach {
        whole: project_access(policy, member).allowed(),
        editable,
        total,
        library,
    }
}
/// Short access line; counting layers is skipped where the rank decides.
fn member_summary(project: &ProjectV2, policy: &RoomPolicy, member: &Member) -> String {
    if member.banned || matches!(member.rank, Rank::Owner | Rank::Viewer) {
        let none = Reach {
            whole: false,
            editable: 0,
            total: 0,
            library: 0,
        };
        return reach_text(member, &none);
    }
    reach_text(member, &reach(project, policy, member))
}
fn reach_text(member: &Member, reach: &Reach) -> String {
    if member.banned {
        return "Banned · can't open the room".into();
    }
    match member.rank {
        Rank::Owner => "Edits everything".into(),
        Rank::Viewer => "Watches only".into(),
        _ if reach.whole => "Edits the whole project".into(),
        _ if reach.editable == 0 && reach.library == 0 => {
            if member.rank == Rank::Admin {
                "Moderates · nothing assigned for editing".into()
            } else {
                "Nothing assigned yet · can't edit".into()
            }
        }
        _ => format!("Edits {} of {} layers", reach.editable, reach.total),
    }
}
/// Rank defaults first (outlined), then personal assignments (accent).
fn access_chips(
    ui: &mut Ui,
    pal: &Palette,
    project: &ProjectV2,
    policy: &RoomPolicy,
    member: &Member,
) {
    if member.rank == Rank::Viewer || member.rank == Rank::Owner || member.banned {
        return;
    }
    let defaults = policy.rank_defaults(member.rank);
    if defaults.is_empty() && member.grants.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for grant in defaults {
            chip(
                ui,
                &format!("{} · default", scope_label(project, &grant.scope)),
                pal.dim,
                false,
            )
            .on_hover_text(format!(
                "{} Comes from the room's {} defaults.",
                scope_hint(&grant.scope),
                rank_text(member.rank)
            ));
        }
        for grant in &member.grants {
            chip(ui, &scope_label(project, &grant.scope), pal.link, false).on_hover_text(format!(
                "{} Assigned by the owner.",
                scope_hint(&grant.scope)
            ));
        }
    });
}
fn coverage_text(access: Access, policy_mode: bool) -> Option<&'static str> {
    if policy_mode {
        return access.rank_default.then_some("included");
    }
    if access.assigned {
        Some("included")
    } else if access.rank_default {
        Some("rank default")
    } else {
        None
    }
}
fn toggle_grant(grants: &mut Vec<Grant>, scope: Scope, on: bool) {
    grants.retain(|g| g.scope != scope);
    if on {
        if scope == Scope::Project {
            grants.clear();
        }
        grants.push(Grant { scope });
    }
}
/// A project-aware assignment tree: real layer names, timeline folders,
/// symbols and library paths. Ticking a folder covers everything inside it.
#[allow(clippy::too_many_arguments)]
fn grant_picker(
    ui: &mut Ui,
    pal: &Palette,
    salt: &str,
    project: &ProjectV2,
    policy: &RoomPolicy,
    preview: &Member,
    grants: &mut Vec<Grant>,
    filter: &mut String,
    policy_mode: bool,
) {
    let whole = grants.iter().any(|g| g.scope == Scope::Project);
    let mut all = whole;
    if ui
        .checkbox(&mut all, "Whole project")
        .on_hover_text(scope_hint(&Scope::Project))
        .changed()
    {
        toggle_grant(grants, Scope::Project, all);
    }
    if whole {
        ui.label(
            RichText::new("Covers every layer, symbol and setting. Untick to choose parts.")
                .small()
                .color(pal.dim),
        );
        return;
    }
    ui.add(
        egui::TextEdit::singleline(filter)
            .id(egui::Id::new(("online_grant_filter", salt)))
            .hint_text("Filter layers and folders")
            .desired_width(f32::INFINITY),
    );
    let needle = filter.trim().to_lowercase();
    let mut q0rgs: Vec<&q0s_format::v2::Q0rg> = project.q0rgs.iter().collect();
    let entry = project.meta.entry_q0rg_id;
    q0rgs.sort_by(|a, b| {
        (b.q0rg_id == entry)
            .cmp(&(a.q0rg_id == entry))
            .then(a.name.cmp(&b.name))
    });
    egui::ScrollArea::vertical()
        .id_source(("online_grant_tree", salt))
        .max_height(260.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for q0rg in q0rgs {
                let rows = timeline_rows(project, q0rg);
                let shown: Vec<_> = rows
                    .iter()
                    .filter(|row| needle.is_empty() || row.1.to_lowercase().contains(&needle))
                    .collect();
                if shown.is_empty() {
                    continue;
                }
                let assigned = grants
                    .iter()
                    .filter(|g| match g.scope {
                        Scope::Layer { q0rg_id, .. } | Scope::LayerFolder { q0rg_id, .. } => {
                            q0rg_id == q0rg.q0rg_id
                        }
                        _ => false,
                    })
                    .count();
                let mut title = if q0rg.q0rg_id == entry {
                    format!("{} · main timeline", q0rg.name)
                } else {
                    format!("{} · symbol", q0rg.name)
                };
                if assigned > 0 {
                    title.push_str(&format!(" · {assigned} ticked"));
                }
                let mut header = egui::CollapsingHeader::new(title)
                    .id_source(("online_grant_q0rg", salt, q0rg.q0rg_id))
                    .default_open(q0rg.q0rg_id == entry || assigned > 0);
                if !needle.is_empty() {
                    header = header.open(Some(true));
                }
                header.show(ui, |ui| {
                    let folder = q0online::q0rg_library_folder(project, q0rg.q0rg_id);
                    if q0rg.q0rg_id != entry && !folder.is_empty() {
                        ui.label(
                            RichText::new(format!("in library {folder}/"))
                                .small()
                                .color(pal.dim),
                        );
                    }
                    for (layer_id, name, is_folder, depth) in shown {
                        let scope = if *is_folder {
                            Scope::LayerFolder {
                                q0rg_id: q0rg.q0rg_id,
                                layer_id: *layer_id,
                            }
                        } else {
                            Scope::Layer {
                                q0rg_id: q0rg.q0rg_id,
                                layer_id: *layer_id,
                            }
                        };
                        let explicit = grants.iter().any(|g| g.scope == scope);
                        let access =
                            layer_access(project, policy, preview, q0rg.q0rg_id, *layer_id);
                        ui.horizontal(|ui| {
                            ui.add_space(if needle.is_empty() {
                                *depth as f32 * 14.0
                            } else {
                                0.0
                            });
                            let mut on = explicit;
                            let label = if *is_folder {
                                format!("▸ {name}/")
                            } else {
                                name.clone()
                            };
                            let response = ui
                                .checkbox(&mut on, label)
                                .on_hover_text(scope_hint(&scope));
                            if response.changed() {
                                toggle_grant(grants, scope.clone(), on);
                            }
                            if !explicit {
                                if let Some(text) = coverage_text(access, policy_mode) {
                                    ui.label(
                                        RichText::new(format!("✓ {text}")).small().color(pal.dim),
                                    );
                                }
                            }
                        });
                    }
                });
            }
            let folders = &project.runtime.library_folders;
            let shown: Vec<&String> = folders
                .iter()
                .filter(|path| needle.is_empty() || path.to_lowercase().contains(&needle))
                .collect();
            let library_ticked = grants
                .iter()
                .filter(|g| matches!(g.scope, Scope::LibraryFolder { .. }))
                .count();
            let mut header = egui::CollapsingHeader::new(if library_ticked > 0 {
                format!("Library folders · {library_ticked} ticked")
            } else {
                "Library folders".to_string()
            })
            .id_source(("online_grant_library", salt))
            .default_open(library_ticked > 0);
            if !needle.is_empty() && !shown.is_empty() {
                header = header.open(Some(true));
            }
            header.show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Library access covers symbols and shared artwork everywhere they're used.",
                    )
                    .small()
                    .color(pal.dim),
                );
                let mut paths: Vec<&str> = vec![""];
                let mut sorted: Vec<&str> = shown.iter().map(|p| p.as_str()).collect();
                sorted.sort_unstable();
                paths.extend(sorted);
                for path in paths {
                    let scope = Scope::LibraryFolder { path: path.into() };
                    let explicit = grants.iter().any(|g| g.scope == scope);
                    let access = library_access(policy, preview, path);
                    let depth = if path.is_empty() {
                        0
                    } else {
                        path.matches('/').count()
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 14.0);
                        let mut on = explicit;
                        let label = if path.is_empty() {
                            "Entire library".to_string()
                        } else {
                            format!("{}/", path.rsplit('/').next().unwrap_or(path))
                        };
                        if ui
                            .checkbox(&mut on, label)
                            .on_hover_text(scope_hint(&scope))
                            .changed()
                        {
                            toggle_grant(grants, scope.clone(), on);
                        }
                        if !explicit {
                            if let Some(text) = coverage_text(access, policy_mode) {
                                ui.label(RichText::new(format!("✓ {text}")).small().color(pal.dim));
                            }
                        }
                    });
                }
            });
        });
}

// --- the panel -----------------------------------------------------------------

impl Online {
    /// Label for the menu bar: live state and anything waiting for you.
    pub fn menu_label(&self) -> String {
        if self.room.is_some() {
            return if self.connection.is_some() {
                "Online · reconnecting".into()
            } else {
                "Online · live".into()
            };
        }
        let waiting = self.invites.len()
            + self.chat.unread.max(0) as usize
            + self
                .friends
                .iter()
                .filter(|f| f.status == "incoming")
                .count();
        if waiting > 0 {
            format!("Online ({waiting})")
        } else {
            "Online".into()
        }
    }

    pub(super) fn show_panel(&mut self, app: &mut EditorApp, ctx: &Context) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        let screen = ctx.screen_rect();
        let mut out = Out::default();
        egui::Window::new("Online")
            .id(egui::Id::new("q0editor_online"))
            .open(&mut open)
            .default_pos(egui::pos2(screen.right() - 410.0, screen.top() + 70.0))
            .default_width(380.0)
            .default_height((screen.height() - 140.0).clamp(420.0, 820.0))
            .min_width(300.0)
            .resizable(true)
            .collapsible(true)
            .constrain(true)
            .show(ctx, |ui| {
                let pal = Palette::new(app, ui);
                if self.user.is_none() {
                    self.signed_out_view(ui, &pal, &mut out);
                } else {
                    self.header(ui, &pal, &mut out);
                    ui.separator();
                    self.tabs(ui);
                    ui.add_space(2.0);
                    // room for the whole message below the scrolling content
                    let footer = self.notice.as_ref().map_or(0.0, |notice| {
                        let font = egui::TextStyle::Body.resolve(ui.style());
                        let width = (ui.available_width() - 30.0).max(60.0);
                        let galley = ui.fonts(|fonts| {
                            fonts.layout(notice.text.clone(), font, Color32::WHITE, width)
                        });
                        galley.size().y + 22.0
                    });
                    egui::ScrollArea::vertical()
                        .id_source("online_panel_scroll")
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - footer).max(120.0))
                        .stick_to_bottom(matches!(self.view.tab, Tab::Chat))
                        .show(ui, |ui| match self.view.tab {
                            Tab::Room => self.room_view(app, ui, &pal, &mut out),
                            Tab::Chat => self.chat_view(app, ui, &pal, &mut out),
                            Tab::Rooms => self.rooms_view(app, ui, &pal, &mut out),
                            Tab::Friends => self.friends_view(ui, &pal, &mut out),
                            Tab::Messages => self.messages_view(ui, &pal, &mut out),
                        });
                }
                self.footer(ui, &pal);
            });
        self.open = open;
        self.apply(app, ctx, out);
    }

    fn apply(&mut self, app: &mut EditorApp, ctx: &Context, out: Out) {
        if out.cancel_sign_in {
            self.cancel_sign_in();
        }
        if out.sign_in {
            self.start_sign_in(ctx);
        }
        if let Some((text, tone)) = out.say {
            self.say(text, tone);
        }
        for command in out.send {
            self.send(command);
        }
        for id in out.avatars {
            self.request_avatar(id);
        }
        if let Some(follow) = out.follow {
            self.follow = Some(follow);
        }
        if let Some(redo) = out.history {
            self.request_history(redo);
        }
        if let Some(id) = out.message {
            self.view.tab = Tab::Messages;
            self.open_thread(id);
        }
        if out.open_join {
            self.view.hold_join = false;
            self.open_pending_join(app);
        }
        if out.open_draft {
            if let Some(project) = self.rejected_drafts.pop() {
                let draft = app.preserve_online_draft(None, project);
                app.open_preserved_online_draft(draft);
                self.say("opened your unshared change as a local copy", Tone::Info);
            } else if let Some(draft) = self.recovery.take() {
                app.open_preserved_online_draft(draft);
                self.say("opened your recovered change in its tab", Tone::Info);
            }
        } else if out.leave && self.room.is_some() {
            self.leave();
            self.refresh();
        }
        if out.logout {
            if self.room.is_some() {
                self.leave();
            }
            self.send(Command::Logout);
        }
    }

    fn footer(&mut self, ui: &mut Ui, pal: &Palette) {
        let Some(notice) = self.notice.clone() else {
            return;
        };
        ui.separator();
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Dismiss this message")
                    .clicked()
                {
                    self.notice = None;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&notice.text).color(pal.tone(notice.tone)))
                            .wrap(true),
                    );
                });
            });
        });
    }

    // --- signed out ----------------------------------------------------------

    fn signed_out_view(&mut self, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        ui.label(RichText::new("Work together on q0id").strong().size(17.0));
        ui.label(
            RichText::new(
                "Share a document with friends, give each person the layers they work on, and see edits arrive live.",
            )
            .color(pal.dim),
        );
        ui.add_space(8.0);
        if self.login.is_some() {
            self.sign_in_controls(ui);
        } else if let Some(message) = self.login_ended.clone() {
            card(ui, pal, |ui| {
                ui.label(RichText::new(message).color(pal.warn));
                ui.label(
                    RichText::new("Nothing was connected. You can start a new sign-in.")
                        .small()
                        .color(pal.dim),
                );
                ui.horizontal(|ui| {
                    if primary(ui, pal, "Try again", !self.busy("login")).clicked() {
                        out.sign_in = true;
                    }
                    if ui.button("Cancel").clicked() {
                        out.cancel_sign_in = true;
                    }
                });
            });
        } else {
            let starting = self.busy("login");
            ui.horizontal(|ui| {
                if primary(ui, pal, "Sign in with q0id", !starting)
                    .on_hover_text("Opens q0id.com in your browser to approve this editor")
                    .clicked()
                {
                    out.sign_in = true;
                }
                if starting {
                    ui.spinner();
                    ui.label(RichText::new("Asking the server for a code…").color(pal.dim));
                }
            });
            ui.label(
                RichText::new("You approve it in your browser. q0editor never sees your password.")
                    .small()
                    .color(pal.dim),
            );
        }
        ui.add_space(8.0);
        egui::CollapsingHeader::new(RichText::new("Server").small())
            .id_source("online_server")
            .show(ui, |ui| {
                let editable = self.transport.is_none();
                ui.add_enabled(
                    editable,
                    egui::TextEdit::singleline(&mut self.server)
                        .hint_text("https://q0id.com")
                        .desired_width(f32::INFINITY),
                )
                .on_disabled_hover_text("Cancel the current sign-in to change the server");
                ui.label(
                    RichText::new(
                        "Use https. Plain http only works for a local development server.",
                    )
                    .small()
                    .color(pal.dim),
                );
            });
        ui.label(
            RichText::new("You stay signed in until you sign out or close q0editor.")
                .small()
                .color(pal.dim),
        );
    }

    /// The waiting-for-approval step. The code stays selectable for Ctrl+C,
    /// and every way to reach the same request is one click away.
    pub(super) fn sign_in_controls(&mut self, ui: &mut Ui) {
        let Some(login) = self.login.clone() else {
            return;
        };
        let dim = ui.visuals().weak_text_color();
        ui.label(RichText::new("Approve this editor in your browser").strong());
        ui.label(
            RichText::new(
                "q0id.com opened in your browser. Check that it shows this code, then choose Connect.",
            )
            .color(dim),
        );
        ui.add_space(4.0);
        // An immutable TextBuffer keeps the code selectable and supports
        // Ctrl+C without allowing accidental edits to the server's code.
        let mut displayed_code = login.code.as_str();
        ui.add(
            egui::TextEdit::singleline(&mut displayed_code)
                .id_source("online_confirmation_code")
                .font(egui::FontId::monospace(22.0))
                .horizontal_align(Align::Center)
                .desired_width(f32::INFINITY),
        )
        .on_hover_text("Select and press Ctrl+C, or use Copy code");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Copy code").clicked() {
                ui.ctx().copy_text(login.code.clone());
                self.say("confirmation code copied", Tone::Success);
            }
            if ui
                .button("Open browser again")
                .on_hover_text("Opens the same approval page; the code doesn't change")
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(login.url.clone()));
                self.say("opened the approval page again", Tone::Info);
            }
            if ui
                .button("Copy sign-in link")
                .on_hover_text("Paste it into any browser where you're signed in to q0id")
                .clicked()
            {
                ui.ctx().copy_text(login.url.clone());
                self.say("sign-in link copied", Tone::Success);
            }
        });
        ui.add_space(4.0);
        let left = login
            .expires
            .saturating_duration_since(Instant::now())
            .as_secs();
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new(format!(
                    "Waiting for approval · expires in {}:{:02}",
                    left / 60,
                    left % 60
                ))
                .color(dim),
            );
        });
        if ui.button("Cancel sign-in").clicked() {
            self.cancel_sign_in();
        }
    }

    // --- signed in -----------------------------------------------------------

    fn header(&mut self, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        let user = self.user.clone().unwrap_or_default();
        if !self.rejected_drafts.is_empty()
            && ui
                .button(format!(
                    "Open unshared copy ({})",
                    self.rejected_drafts.len()
                ))
                .clicked()
        {
            out.open_draft = true;
        }
        let host = self
            .server
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_owned();
        ui.horizontal(|ui| {
            avatar(ui, &self.avatars, &user, 30.0, out);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(&user.username).strong());
                ui.label(
                    RichText::new(format!("signed in on {host}"))
                        .small()
                        .color(pal.dim),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("⋯", |ui| {
                    if ui.button("Refresh lists").clicked() {
                        out.send.push(Command::Refresh);
                        ui.close_menu();
                    }
                    if ui.button("Open q0id online in browser").clicked() {
                        ui.ctx()
                            .open_url(egui::OpenUrl::new_tab(format!("{}/online", self.server)));
                        ui.close_menu();
                    }
                    ui.separator();
                    let label = if self.room.is_some() {
                        "Leave room and sign out"
                    } else {
                        "Sign out"
                    };
                    if ui
                        .add_enabled(!self.busy("logout"), egui::Button::new(label))
                        .on_hover_text("Ends this editor's q0id sign-in. Your documents stay open.")
                        .clicked()
                    {
                        out.logout = true;
                        ui.close_menu();
                    }
                })
                .response
                .on_hover_text("Account");
                let (text, color, filled) = match &self.room {
                    Some(_) if self.connection.is_some() => ("reconnecting", pal.warn, false),
                    Some(room) if room.pending.is_some() || room.changed_at.is_some() => {
                        ("syncing", pal.link, false)
                    }
                    Some(_) => ("live", pal.ok, true),
                    None => ("online", pal.dim, false),
                };
                let response = ui.label(RichText::new(text).small().color(color));
                dot(ui, color, filled);
                if let Some(message) = &self.connection {
                    response.on_hover_text(message);
                }
            });
        });
    }

    fn tabs(&mut self, ui: &mut Ui) {
        if self.room.is_none() && matches!(self.view.tab, Tab::Room | Tab::Chat) {
            self.view.tab = Tab::Rooms;
        }
        let invites = self.invites.len();
        let requests = self
            .friends
            .iter()
            .filter(|f| f.status == "incoming")
            .count();
        ui.horizontal_wrapped(|ui| {
            if self.room.is_some() {
                ui.selectable_value(&mut self.view.tab, Tab::Room, "Room")
                    .on_hover_text("The room you're in: people, access and settings");
                ui.selectable_value(&mut self.view.tab, Tab::Chat, "Chat")
                    .on_hover_text("Talk to everyone in the room");
            }
            let rooms = if invites > 0 {
                format!("Rooms ({invites})")
            } else {
                "Rooms".into()
            };
            ui.selectable_value(&mut self.view.tab, Tab::Rooms, rooms)
                .on_hover_text("Invitations, your rooms, and starting a new one");
            let friends = if requests > 0 {
                format!("Friends ({requests})")
            } else {
                "Friends".into()
            };
            ui.selectable_value(&mut self.view.tab, Tab::Friends, friends)
                .on_hover_text("Find people, answer requests and invite friends");
            let unread = self.chat.unread.max(0);
            let messages = if unread > 0 {
                format!("Messages ({unread})")
            } else {
                "Messages".into()
            };
            if ui
                .selectable_value(&mut self.view.tab, Tab::Messages, messages)
                .on_hover_text("Direct messages, also on q0id.com")
                .clicked()
            {
                // the list is refreshed every time you come back to it
                self.chat.conversations_loaded = false;
                self.chat.requested = false;
            }
            if self.busy("refresh") {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spinner().on_hover_text("Refreshing");
                });
            }
        });
    }

    // --- rooms (lobby) -------------------------------------------------------

    fn rooms_view(&mut self, app: &EditorApp, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        let now = self.server_now();
        let joining = self.busy("join");
        if let Some(pending) = &self.pending_join {
            card(ui, pal, |ui| {
                ui.label(RichText::new(format!("“{}” is ready to open", pending.name)).strong());
                ui.label(
                    RichText::new(
                        "Finish the current import or export, then open it in its own tab.",
                    )
                    .small()
                    .color(pal.dim),
                );
                if primary(ui, pal, "Open room document", true).clicked() {
                    out.open_join = true;
                }
            });
        }
        if self.room.is_some() {
            card(ui, pal, |ui| {
                ui.label(
                    RichText::new("You're in a room. Leave it to join another or start a new one.")
                        .small()
                        .color(pal.dim),
                );
            });
        }

        if !self.invites.is_empty() {
            heading(
                ui,
                pal,
                "Invitations",
                "Accepting opens the room in a new tab. Your open documents stay as they are.",
            );
            for invite in self.invites.clone() {
                let inviter = self.person(invite.inviter_id);
                let rank = invite
                    .member
                    .as_ref()
                    .map(|m| m.rank)
                    .unwrap_or(Rank::Viewer);
                card(ui, pal, |ui| {
                    ui.horizontal(|ui| {
                        avatar(ui, &self.avatars, &inviter, ROW_AVATAR, out);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.add(
                                egui::Label::new(RichText::new(&invite.room_name).strong())
                                    .truncate(true),
                            );
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    RichText::new(format!("from {} · as", invite.inviter_name))
                                        .small()
                                        .color(pal.dim),
                                );
                                rank_chip(ui, pal, rank);
                                ui.label(
                                    RichText::new(until(now, invite.expires_at))
                                        .small()
                                        .color(pal.dim),
                                );
                            });
                        });
                    });
                    ui.horizontal(|ui| {
                        let declining = self.busy(&format!("decline:{}", invite.id));
                        let can_join = !joining && self.room.is_none();
                        if primary(ui, pal, "Join", can_join)
                            .on_disabled_hover_text(if self.room.is_some() {
                                "Leave your current room first"
                            } else {
                                "Opening a room…"
                            })
                            .clicked()
                        {
                            out.send.push(Command::AcceptInvite {
                                id: invite.id.clone(),
                            });
                        }
                        if ui
                            .add_enabled(!declining, egui::Button::new("Decline"))
                            .clicked()
                        {
                            out.send.push(Command::DeclineInvite {
                                id: invite.id.clone(),
                            });
                        }
                        if joining || declining {
                            ui.spinner();
                        }
                    });
                });
            }
        }

        heading(
            ui,
            pal,
            "Start a room",
            "Shares the open document. Embed external files first.",
        );
        let project_name = app
            .has_active_project()
            .then(|| app.state.project.meta.name.clone());
        if !self.view.room_name_edited {
            self.view.room_name = project_name.clone().unwrap_or_default();
        }
        card(ui, pal, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.view.room_name)
                    .hint_text("Room name")
                    .char_limit(100)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                self.view.room_name_edited = true;
            }
            let name = self.view.room_name.trim().to_owned();
            let can_create =
                project_name.is_some() && self.room.is_none() && !joining && !name.is_empty();
            let submit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                let reason = if project_name.is_none() {
                    "Open or create a project first"
                } else if self.room.is_some() {
                    "Leave your current room first"
                } else if name.is_empty() {
                    "Give the room a name"
                } else {
                    "Starting the room…"
                };
                if (primary(ui, pal, "Start room from this document", can_create)
                    .on_disabled_hover_text(reason)
                    .clicked()
                    || (submit && can_create))
                    && app.has_active_project()
                {
                    out.send.push(Command::Create {
                        name,
                        project: Box::new(app.state.project.clone()),
                    });
                }
                if joining {
                    ui.spinner();
                }
            });
            ui.label(
                RichText::new("You become the owner. Invite friends next: editors can draw right away, viewers watch.")
                    .small()
                    .color(pal.dim),
            );
        });

        ui.horizontal(|ui| {
            heading(ui, pal, "Your rooms", "");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let refreshing = self.busy("refresh");
                if ui
                    .add_enabled(!refreshing, egui::Button::new("⟳").small())
                    .on_hover_text("Refresh rooms, friends and invitations")
                    .clicked()
                {
                    out.send.push(Command::Refresh);
                }
            });
        });
        if self.lists_at.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading your rooms…").color(pal.dim));
            });
        } else if self.rooms.is_empty() {
            ui.label(
                RichText::new("No rooms yet. Start one above, or accept an invitation.")
                    .color(pal.dim),
            );
        }
        let me = self.actor();
        let closing = self.busy("close");
        for room in self.rooms.clone() {
            let current = self.room.as_ref().is_some_and(|r| r.id == room.id);
            card(ui, pal, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&room.name).strong()).truncate(true),
                            );
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            rank_chip(ui, pal, room.rank);
                            if room.closed {
                                chip(ui, "closed", pal.dim, false);
                            }
                            if current {
                                chip(ui, "you're here", pal.ok, true);
                            }
                            let owner = if Some(room.owner_id) == me {
                                "your room".to_string()
                            } else {
                                format!("by {}", room.owner_name)
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{owner} · {} · active {}",
                                    plural(room.member_count.max(0) as usize, "member", "members"),
                                    ago(now, room.updated_at)
                                ))
                                .small()
                                .color(pal.dim),
                            );
                        });
                    });
                });
                ui.horizontal(|ui| {
                    if current {
                        if ui.button("Show room").clicked() {
                            self.view.tab = Tab::Room;
                        }
                    } else if room.closed {
                        if Some(room.owner_id) == me {
                            if ui
                                .add_enabled(!closing, egui::Button::new("Reopen"))
                                .on_hover_text("Lets members open this room again")
                                .clicked()
                            {
                                out.send.push(Command::CloseRoom {
                                    room: room.id.clone(),
                                    closed: false,
                                });
                            }
                            // open rooms are deleted from their settings, never
                            // from a list where the button sat first in line
                            if armed_button(
                                ui,
                                &mut self.view,
                                pal,
                                &format!("delete:{}", room.id),
                                "Delete",
                                "Delete this room permanently?",
                                !self.busy.contains("delete-room"),
                            ) {
                                out.send.push(Command::DeleteRoom {
                                    room: room.id.clone(),
                                });
                            }
                        } else {
                            ui.label(RichText::new("Closed").small().color(pal.dim));
                        }
                    } else {
                        let can_join = !joining && self.room.is_none();
                        if primary(ui, pal, "Join", can_join)
                            .on_disabled_hover_text(if self.room.is_some() {
                                "Leave your current room first"
                            } else {
                                "Opening a room…"
                            })
                            .on_hover_text("Opens the room in a new tab")
                            .clicked()
                        {
                            out.send.push(Command::Join {
                                id: room.id.clone(),
                            });
                        }
                    }
                    if !room.closed
                        && ui
                            .button("Copy code")
                            .on_hover_text("Room codes let members join from another editor")
                            .clicked()
                    {
                        copy(ui, &room.id, out, "room code");
                    }
                });
            });
        }

        egui::CollapsingHeader::new("Join with a room code")
            .id_source("online_join_code")
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Members can paste a room code from the website or a friend.")
                        .small()
                        .color(pal.dim),
                );
                // button first from the right, field fills the rest: subtracting a
                // guessed button width made the window grow a little every frame
                one_line(ui, |ui| {
                    let code = self.view.join_code.trim().to_owned();
                    let valid = code.len() == 32 && code.bytes().all(|b| b.is_ascii_hexdigit());
                    let enabled = valid && !joining && self.room.is_none();
                    let clicked = ui
                        .add_enabled(enabled, egui::Button::new("Join"))
                        .on_disabled_hover_text(if self.room.is_some() {
                            "Leave your current room first"
                        } else {
                            "Paste a 32-character room code"
                        })
                        .clicked();
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.view.join_code)
                            .hint_text("32-character room code")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                    let submit =
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (clicked || (submit && enabled)) && valid {
                        out.send.push(Command::Join {
                            id: code.to_ascii_lowercase(),
                        });
                    }
                });
            });
    }

    // --- friends -------------------------------------------------------------

    fn friends_view(&mut self, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        let now = self.server_now();
        heading(
            ui,
            pal,
            "Find people",
            "Friends can invite each other to rooms.",
        );
        let searching = self.busy("search");
        one_line(ui, |ui| {
            let query = self.view.search.trim().to_owned();
            let enabled = query.chars().count() >= 2 && !searching;
            let clicked = ui
                .add_enabled(enabled, egui::Button::new("Search"))
                .on_disabled_hover_text("Type at least 2 characters")
                .clicked();
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.view.search)
                    .hint_text("Username")
                    .char_limit(20)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() && self.view.search.trim().is_empty() {
                self.results = None;
            }
            let query = self.view.search.trim().to_owned();
            let enabled = query.chars().count() >= 2 && !searching;
            let submit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if clicked || (submit && enabled) {
                out.send.push(Command::Search { query });
            }
        });
        if let Some(results) = self.results.clone() {
            if results.is_empty() {
                ui.label(
                    RichText::new("Nobody by that name. Usernames match from the start.")
                        .small()
                        .color(pal.dim),
                );
            }
            for person in results {
                let known = self
                    .friends
                    .iter()
                    .find(|f| f.person.id == person.id)
                    .map(|f| f.status.clone());
                let busy = self.busy(&format!("friend:{}", person.id));
                self.person_row(ui, pal, out, &person, None, |ui, out| {
                    match known.as_deref() {
                        Some("accepted") => {
                            chip(ui, "friends", pal.dim, false);
                        }
                        Some("outgoing") => {
                            chip(ui, "requested", pal.dim, false);
                        }
                        Some(_) => {
                            if primary(ui, pal, "Accept", !busy).clicked() {
                                out.send.push(Command::Friend {
                                    user_id: person.id,
                                    action: FriendAction::Accept,
                                });
                            }
                        }
                        None => {
                            if primary(ui, pal, "Add friend", !busy).clicked() {
                                out.send.push(Command::Friend {
                                    user_id: person.id,
                                    action: FriendAction::Request,
                                });
                            }
                        }
                    }
                });
            }
        }

        let requests: Vec<FriendRow> = self
            .friends
            .iter()
            .filter(|f| f.status != "accepted")
            .cloned()
            .collect();
        if !requests.is_empty() {
            heading(ui, pal, "Requests", "");
            for request in requests {
                let busy = self.busy(&format!("friend:{}", request.person.id));
                let incoming = request.status == "incoming";
                let sub = if incoming {
                    format!("wants to be friends · {}", ago(now, request.since))
                } else {
                    format!("request sent · {}", ago(now, request.since))
                };
                let id = request.person.id;
                self.person_row(ui, pal, out, &request.person, Some(sub), |ui, out| {
                    if incoming {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Decline"))
                            .clicked()
                        {
                            out.send.push(Command::Friend {
                                user_id: id,
                                action: FriendAction::Decline,
                            });
                        }
                        if primary(ui, pal, "Accept", !busy).clicked() {
                            out.send.push(Command::Friend {
                                user_id: id,
                                action: FriendAction::Accept,
                            });
                        }
                    } else if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                        out.send.push(Command::Friend {
                            user_id: id,
                            action: FriendAction::Remove,
                        });
                    }
                });
            }
        }

        let friends: Vec<FriendRow> = self
            .friends
            .iter()
            .filter(|f| f.status == "accepted")
            .cloned()
            .collect();
        let online = friends
            .iter()
            .filter(|f| now - f.person.last_seen < 300)
            .count();
        heading(
            ui,
            pal,
            "Friends",
            &if friends.is_empty() {
                String::new()
            } else {
                format!("{online} online · {} total", friends.len())
            },
        );
        if self.lists_at.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").color(pal.dim));
            });
        } else if friends.is_empty() {
            ui.label(RichText::new("No friends yet. Search for a username above.").color(pal.dim));
        }
        let room = self.room.as_ref().map(|r| {
            (
                r.id.clone(),
                r.name.clone(),
                r.members.iter().map(|m| m.user_id).collect::<Vec<_>>(),
                r.invites.iter().map(|i| i.invitee_id).collect::<Vec<_>>(),
            )
        });
        let can_invite = self.my_member().is_some_and(|m| {
            !m.banned
                && (m.rank >= Rank::Admin
                    || self
                        .room
                        .as_ref()
                        .is_some_and(|r| r.policy.members_can_invite))
        });
        let mut sorted = friends;
        sorted.sort_by(|a, b| {
            (now - b.person.last_seen < 300)
                .cmp(&(now - a.person.last_seen < 300))
                .then(a.person.username.cmp(&b.person.username))
        });
        for friend in sorted {
            let id = friend.person.id;
            let online = now - friend.person.last_seen < 300;
            let sub = if online {
                "online now".to_string()
            } else {
                format!("seen {}", ago(now, friend.person.last_seen))
            };
            let busy = self.busy(&format!("friend:{id}"));
            let inviting = self.busy(&format!("invite:{id}"));
            let room = room.clone();
            let pal_ref = pal;
            let invite_rank = self.view.invite_rank;
            self.person_row_with_view(
                ui,
                pal,
                out,
                &friend.person,
                Some(sub),
                online,
                |ui, out, view| {
                    ui.menu_button("⋯", |ui| {
                        if ui.button("Message").clicked() {
                            out.message = Some(id);
                            ui.close_menu();
                        }
                        if armed_button(
                            ui,
                            view,
                            pal_ref,
                            &format!("remove:{id}"),
                            "Remove friend",
                            "Remove for good?",
                            !busy,
                        ) {
                            out.send.push(Command::Friend {
                                user_id: id,
                                action: FriendAction::Remove,
                            });
                            ui.close_menu();
                        }
                        if armed_button(
                            ui,
                            view,
                            pal_ref,
                            &format!("block:{id}"),
                            "Block",
                            "Block them?",
                            !busy,
                        ) {
                            out.send.push(Command::Friend {
                                user_id: id,
                                action: FriendAction::Block,
                            });
                            ui.close_menu();
                        }
                        ui.label(
                            RichText::new(
                                "Blocked people can't find you, invite you or send requests.",
                            )
                            .small()
                            .color(pal_ref.dim),
                        );
                    })
                    .response
                    .on_hover_text("More");
                    if let Some((room_id, room_name, members, invited)) = &room {
                        if members.contains(&id) {
                            chip(ui, "in room", pal_ref.dim, false);
                        } else if invited.contains(&id) {
                            chip(ui, "invited", pal_ref.dim, false);
                        } else if can_invite
                            && ui
                                .add_enabled(!inviting, egui::Button::new("Invite"))
                                .on_hover_text(format!(
                                    "Invite to “{room_name}” as {}",
                                    rank_text(invite_rank)
                                ))
                                .clicked()
                        {
                            out.send.push(Command::Invite {
                                room: room_id.clone(),
                                user_id: id,
                                rank: invite_rank,
                            });
                        }
                    }
                },
            );
        }

        if !self.blocked.is_empty() {
            egui::CollapsingHeader::new(format!("Blocked · {}", self.blocked.len()))
                .id_source("online_blocked")
                .show(ui, |ui| {
                    for person in self.blocked.clone() {
                        let busy = self.busy(&format!("friend:{}", person.id));
                        let id = person.id;
                        self.person_row(ui, pal, out, &person, None, |ui, out| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Unblock"))
                                .clicked()
                            {
                                out.send.push(Command::Friend {
                                    user_id: id,
                                    action: FriendAction::Unblock,
                                });
                            }
                        });
                    }
                });
        }
    }

    fn person_row(
        &mut self,
        ui: &mut Ui,
        pal: &Palette,
        out: &mut Out,
        person: &Person,
        sub: Option<String>,
        actions: impl FnOnce(&mut Ui, &mut Out),
    ) {
        self.person_row_with_view(ui, pal, out, person, sub, false, |ui, out, _| {
            actions(ui, out)
        });
    }
    /// One person: avatar, name and detail on the left, actions on the right.
    /// Actions get room first so names truncate instead of pushing buttons away.
    #[allow(clippy::too_many_arguments)]
    fn person_row_with_view(
        &mut self,
        ui: &mut Ui,
        pal: &Palette,
        out: &mut Out,
        person: &Person,
        sub: Option<String>,
        online: bool,
        actions: impl FnOnce(&mut Ui, &mut Out, &mut View),
    ) {
        let avatars = &self.avatars;
        let view = &mut self.view;
        ui.horizontal(|ui| {
            ui.set_min_height(ROW_AVATAR + 4.0);
            avatar(ui, avatars, person, ROW_AVATAR, out);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                actions(ui, out, view);
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.horizontal(|ui| {
                        if online {
                            dot(ui, pal.ok, true).on_hover_text("Active in the last few minutes");
                        }
                        ui.add(
                            egui::Label::new(RichText::new(&person.username).strong())
                                .truncate(true),
                        );
                    });
                    if let Some(sub) = &sub {
                        ui.add(
                            egui::Label::new(RichText::new(sub).small().color(pal.dim))
                                .truncate(true),
                        );
                    }
                });
            });
        });
    }

    // --- chat and messages --------------------------------------------------------

    fn chat_view(&mut self, app: &EditorApp, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        let now = self.server_now();
        let me = self.actor();
        let key = app
            .settings
            .hotkeys
            .shortcuts(crate::hotkeys::ShortcutCommand::OpenChat)
            .first()
            .map(|chord| chord.display());
        heading(
            ui,
            pal,
            "Room chat",
            &match key {
                Some(key) => format!(
                    "Everyone in the room sees this, here and on q0id.com. Press {key} anywhere to chat over the canvas."
                ),
                None => "Everyone in the room sees this, here and on q0id.com.".into(),
            },
        );
        if self.chat.room.is_empty() {
            ui.label(
                RichText::new("Nothing said yet. Say hi.")
                    .italics()
                    .color(pal.dim),
            );
        }
        let dark = ui.visuals().dark_mode;
        for line in self.chat.room.clone() {
            let name = if Some(line.user_id) == me {
                "you".to_owned()
            } else {
                self.person(line.user_id).username
            };
            let mine = Some(line.user_id) == me;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let who = ui
                    .add(
                        egui::Label::new(
                            RichText::new(format!("{name}:"))
                                .strong()
                                .color(chat::name_colour(line.user_id, dark)),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text(if mine {
                        ago(now, line.created_at)
                    } else {
                        format!("{} · right-click to report", ago(now, line.created_at))
                    });
                if line.removed {
                    ui.label(
                        RichText::new("removed by a moderator")
                            .italics()
                            .color(pal.dim),
                    );
                } else if !line.body.is_empty() {
                    ui.label(&line.body);
                }
                if !mine && !line.removed {
                    report_menu(&who, out, "room_message", line.id);
                }
            });
            if let Some(file) = line.attachment.as_ref().filter(|_| !line.removed) {
                ui.indent(("room_chat_file", line.id), |ui| {
                    self.attachment_ui(ui, pal, file);
                });
            }
        }
        ui.add_space(6.0);
        one_line(ui, |ui| {
            let ready = chat::room_body(&self.chat.room_draft).is_some();
            let clicked = primary(ui, pal, "Send", ready).clicked();
            if ui
                .button("+")
                .on_hover_text("Send a picture, gif or video")
                .clicked()
            {
                if let (Some(path), Some(room)) =
                    (pick_media(), self.room.as_ref().map(|r| r.id.clone()))
                {
                    let body = chat::room_body(&std::mem::take(&mut self.chat.room_draft))
                        .unwrap_or_default();
                    out.send.push(Command::SendFile {
                        to: FileTarget::Room(room),
                        path,
                        body,
                    });
                }
            }
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.chat.room_draft)
                    .id_source("online_room_chat_draft")
                    .hint_text("Message the room")
                    .char_limit(chat::MAX_ROOM_CHARS)
                    .desired_width(f32::INFINITY),
            );
            let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (clicked || entered) && ready {
                let text = std::mem::take(&mut self.chat.room_draft);
                if let Some(room) = self.room.as_ref().map(|r| r.id.clone()) {
                    if let Some(body) = chat::room_body(&text) {
                        out.send.push(Command::Say { room, body });
                    }
                }
                edit.request_focus();
            }
        });
    }

    fn messages_view(&mut self, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        if let Some(with) = self.chat.thread_with {
            self.thread_view(with, ui, pal, out);
            return;
        }
        let now = self.server_now();
        let me = self.actor();
        if !self.chat.conversations_loaded && !self.chat.requested {
            self.chat.requested = true;
            out.send.push(Command::Conversations);
        }
        heading(
            ui,
            pal,
            "Messages",
            "Write to anyone on q0id. The same conversations are on q0id.com/messages.",
        );
        let searching = self.busy("search");
        one_line(ui, |ui| {
            let query = self.chat.find.trim().to_owned();
            let enabled = query.chars().count() >= 2 && !searching;
            let clicked = ui
                .add_enabled(enabled, egui::Button::new("Find"))
                .on_disabled_hover_text("Type at least 2 characters")
                .clicked();
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.chat.find)
                    .id_source("online_dm_find")
                    .hint_text("New message: username")
                    .char_limit(20)
                    .desired_width(f32::INFINITY),
            );
            let submit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if clicked || (submit && enabled) {
                out.send.push(Command::Search { query });
            }
        });
        if !self.chat.find.trim().is_empty() {
            if let Some(results) = self.results.clone() {
                let results: Vec<Person> =
                    results.into_iter().filter(|p| Some(p.id) != me).collect();
                if results.is_empty() {
                    ui.label(RichText::new("Nobody by that name.").small().color(pal.dim));
                }
                for person in results {
                    let id = person.id;
                    self.person_row(ui, pal, out, &person, None, |ui, out| {
                        if primary(ui, pal, "Message", true).clicked() {
                            out.message = Some(id);
                        }
                    });
                }
                ui.separator();
            }
        }
        if !self.chat.conversations_loaded {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").color(pal.dim));
            });
            return;
        }
        if self.chat.conversations.is_empty() {
            ui.label(
                RichText::new(
                    "No conversations yet. Find someone above, or use Message on a friend.",
                )
                .color(pal.dim),
            );
        }
        for conversation in self.chat.conversations.clone() {
            let person = self.person(conversation.user_id);
            let sub = match &conversation.last {
                _ if conversation.blocked => "blocked".to_owned(),
                Some(last) => {
                    let text: String =
                        chat::line_text(&last.body, last.attachment.as_ref(), last.removed)
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                    let text: String = text.chars().take(60).collect();
                    format!(
                        "{}{} · {}",
                        if Some(last.from) == me { "you: " } else { "" },
                        text,
                        ago(now, last.created_at)
                    )
                }
                None => String::new(),
            };
            let id = conversation.user_id;
            let unread = conversation.unread;
            self.person_row(ui, pal, out, &person, Some(sub), |ui, out| {
                let label = if unread > 0 {
                    format!("Open ({unread})")
                } else {
                    "Open".to_owned()
                };
                let button = if unread > 0 {
                    primary(ui, pal, &label, true)
                } else {
                    ui.button(label)
                };
                if button.clicked() {
                    out.message = Some(id);
                }
            });
        }
    }

    fn thread_view(&mut self, with: i64, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        let now = self.server_now();
        let me = self.actor();
        let person = self.person(with);
        ui.horizontal(|ui| {
            if ui.button("‹ All messages").clicked() {
                self.chat.thread_with = None;
                self.chat.conversations_loaded = false;
                self.chat.requested = false;
            }
            avatar(ui, &self.avatars, &person, ROW_AVATAR, out);
            ui.label(RichText::new(&person.username).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("Profile")
                    .on_hover_text("Open their q0id profile in your browser")
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                        "{}/u/{}",
                        self.server, person.username
                    )));
                }
            });
        });
        ui.separator();
        let Some(thread) = self.chat.threads.get(&with).cloned() else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").color(pal.dim));
            });
            return;
        };
        if thread.is_empty() {
            ui.label(
                RichText::new(format!("Say hi to {}.", person.username))
                    .italics()
                    .color(pal.dim),
            );
        }
        for message in &thread {
            let mine = Some(message.from) == me;
            let layout = if mine {
                Layout::right_to_left(Align::TOP)
            } else {
                Layout::left_to_right(Align::TOP)
            };
            ui.with_layout(layout, |ui| {
                // laid out first, so a bubble is exactly as wide as its text
                let (fill, text) = if mine {
                    (pal.accent, pal.on_accent)
                } else {
                    (pal.card, pal.text)
                };
                let pad = vec2(10.0, 6.0);
                let font = egui::TextStyle::Body.resolve(ui.style());
                let wrap = (ui.available_width() * 0.8 - pad.x * 2.0).max(60.0);
                let shown = if message.removed {
                    "removed by a moderator".to_owned()
                } else {
                    message.body.clone()
                };
                if shown.is_empty() {
                    return;
                }
                let galley = ui.fonts(|fonts| fonts.layout(shown, font, text, wrap));
                let (rect, response) =
                    ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::click());
                let corner = |near: bool| if near { 3.0 } else { 12.0 };
                let rounding = egui::Rounding {
                    nw: corner(!mine),
                    ne: corner(mine),
                    sw: 12.0,
                    se: 12.0,
                };
                ui.painter().rect(
                    rect,
                    rounding,
                    fill,
                    Stroke::new(1.0_f32, if mine { fill } else { pal.line }),
                );
                ui.painter().galley(rect.min + pad, galley, text);
                let response = response.on_hover_text(if mine {
                    ago(now, message.created_at)
                } else {
                    format!("{} · right-click to report", ago(now, message.created_at))
                });
                if !mine && !message.removed {
                    report_menu(&response, out, "direct_message", message.id);
                }
            });
            if let Some(file) = message.attachment.as_ref().filter(|_| !message.removed) {
                ui.with_layout(layout, |ui| self.attachment_ui(ui, pal, file));
            }
            ui.add_space(3.0);
        }
        ui.add_space(6.0);
        if self.chat.blocked_with.contains(&with) {
            ui.label(RichText::new("You can't message each other.").color(pal.dim));
            return;
        }
        let edit = ui.add(
            egui::TextEdit::multiline(&mut self.chat.dm_draft)
                .id_source("online_dm_draft")
                .hint_text("Write a message. Enter sends, Shift+Enter starts a new line")
                .char_limit(chat::MAX_DM_CHARS)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        let entered =
            edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        ui.horizontal(|ui| {
            let ready = chat::dm_body(&self.chat.dm_draft).is_some();
            if ui
                .button("+")
                .on_hover_text("Send a picture, gif or video")
                .clicked()
            {
                if let Some(path) = pick_media() {
                    let body =
                        chat::dm_body(&std::mem::take(&mut self.chat.dm_draft)).unwrap_or_default();
                    out.send.push(Command::SendFile {
                        to: FileTarget::Dm(with),
                        path,
                        body,
                    });
                }
            }
            if primary(ui, pal, "Send", ready).clicked() || (entered && ready) {
                let text = std::mem::take(&mut self.chat.dm_draft);
                if let Some(body) = chat::dm_body(&text) {
                    out.send.push(Command::SendDm {
                        user_id: with,
                        body,
                    });
                }
                edit.request_focus();
            } else if entered {
                // Enter on an empty message must not leave a blank line behind
                self.chat.dm_draft = self.chat.dm_draft.trim_end_matches('\n').to_owned();
            }
        });
    }

    /// A picture inline (click to see it full size in the browser), or a
    /// video, which plays in the browser.
    fn attachment_ui(&mut self, ui: &mut Ui, pal: &Palette, file: &chat::Attachment) {
        let url = format!("{}/api/v1/attachments/{}", self.server, file.id);
        let name = if file.name.is_empty() {
            "file"
        } else {
            file.name.as_str()
        };
        if file.is_video() {
            if ui
                .button(format!("▶ {name}"))
                .on_hover_text("Plays in your browser")
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            return;
        }
        match self.preview(&file.id) {
            Some(texture) => {
                let size = texture.size_vec2();
                let scale = (240.0 / size.x).min(180.0 / size.y).min(1.0);
                let response = ui
                    .add(
                        egui::Image::new(egui::load::SizedTexture::new(texture.id(), size * scale))
                            .rounding(4.0)
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(format!("{name} · open full size in your browser"));
                if response.clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
            None => {
                ui.label(
                    RichText::new(format!("picture · {name}"))
                        .small()
                        .color(pal.dim),
                );
            }
        }
    }

    // --- the room you're in ------------------------------------------------------

    fn room_view(&mut self, app: &EditorApp, ui: &mut Ui, pal: &Palette, out: &mut Out) {
        let me = self.actor();
        let now = self.server_now();
        let Online {
            room,
            view,
            people,
            avatars,
            busy,
            recovery,
            rejected_drafts,
            connection,
            friends,
            ..
        } = self;
        let Some(room) = room.as_ref() else {
            ui.label("You're not in a room.");
            return;
        };
        let name_of = |id: i64| {
            people.get(&id).cloned().unwrap_or_else(|| Person {
                id,
                username: format!("member #{id}"),
                ..Person::default()
            })
        };
        let project = &room.confirmed;
        let my = room.member(me).cloned();
        let my_rank = my.as_ref().map(|m| m.rank).unwrap_or(Rank::Viewer);
        let owner = my_rank == Rank::Owner;

        // header: what room, how it's doing, and your two most used actions
        card(ui, pal, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(RichText::new(&room.name).strong().size(16.0)).truncate(true),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (text, color) = if let Some(message) = connection {
                    (format!("Reconnecting · {message}"), pal.warn)
                } else if room.pending.is_some() {
                    ("Sending your edit…".to_string(), pal.link)
                } else if room.changed_at.is_some() {
                    ("Your edit is about to sync".to_string(), pal.link)
                } else {
                    (format!("Live · revision {}", room.revision), pal.ok)
                };
                dot(ui, color, connection.is_none());
                ui.label(RichText::new(text).small().color(color));
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("You are").small().color(pal.dim));
                rank_chip(ui, pal, my_rank);
                if !owner {
                    ui.label(
                        RichText::new(format!("· owner {}", name_of(room.owner_id).username))
                            .small()
                            .color(pal.dim),
                    );
                }
            });
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let can_edit = my_rank != Rank::Viewer;
                if ui
                    .add_enabled(can_edit, egui::Button::new("↶ Undo mine"))
                    .on_hover_text("Undoes your latest edit only. Ctrl+Z does the same while you're in a room.")
                    .on_disabled_hover_text("Viewers have no edits to undo")
                    .clicked()
                {
                    out.history = Some(false);
                }
                if ui
                    .add_enabled(can_edit, egui::Button::new("↷ Redo mine"))
                    .on_hover_text("Redoes your latest undone edit. Ctrl+Y / Ctrl+Shift+Z work too.")
                    .on_disabled_hover_text("Viewers have no edits to redo")
                    .clicked()
                {
                    out.history = Some(true);
                }
                if ui.button("Copy code").on_hover_text("Others can join with this room code").clicked() {
                    copy(ui, &room.id, out, "room code");
                }
                let unsent = room.pending.is_some() || room.changed_at.is_some();
                if unsent {
                    if armed_button(ui, view, pal, "leave", "Leave", "Leave before it syncs?", true) {
                        out.leave = true;
                    }
                } else if ui
                    .button("Leave")
                    .on_hover_text("Disconnect. The document stays open as a local copy; you stay a member.")
                    .clicked()
                {
                    out.leave = true;
                }
            });
        });

        if (recovery.is_some() || !rejected_drafts.is_empty()) && !view.recovery_hidden {
            egui::Frame::none()
                .stroke(Stroke::new(1.0_f32, pal.warn))
                .rounding(6.0)
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("A change of yours wasn't accepted by the room.")
                            .color(pal.warn),
                    );
                    ui.label(
                        RichText::new("Your copy is saved for recovery. Open it when you need it.")
                            .small()
                            .color(pal.dim),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .button("Open local copy")
                            .on_hover_text(
                                "Opens a tab only when you ask; your room stays connected",
                            )
                            .clicked()
                        {
                            out.open_draft = true;
                        }
                        if ui
                            .button("Keep working")
                            .on_hover_text("Hide this message; the unshared copy stays available")
                            .clicked()
                        {
                            view.recovery_hidden = true;
                        }
                    });
                });
        }

        // rooms made before editors could draw by default: say who is stuck
        // and fix it in one click instead of leaving the owner to discover it
        if owner {
            for rank in [Rank::Editor, Rank::Admin] {
                if !room.policy.rank_defaults(rank).is_empty() {
                    continue;
                }
                let stuck: Vec<String> = room
                    .members
                    .iter()
                    .filter(|m| !m.banned && m.rank == rank && m.grants.is_empty())
                    .map(|m| name_of(m.user_id).username)
                    .collect();
                if stuck.is_empty() {
                    continue;
                }
                egui::Frame::none()
                    .stroke(Stroke::new(1.0_f32, pal.warn))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(format!("{} can't edit anything yet", stuck.join(", ")))
                                .color(pal.warn),
                        );
                        ui.label(
                            RichText::new(format!(
                                "This room gives {}s only what you assign. Open a person below to assign layers, or:",
                                rank_text(rank)
                            ))
                            .small()
                            .color(pal.dim),
                        );
                        let label = format!("Let {}s edit everything", rank_text(rank));
                        if primary(ui, pal, &label, !busy.contains("policy")).clicked() {
                            let mut policy = room.policy.clone();
                            let all = vec![Grant { scope: Scope::Project }];
                            if rank == Rank::Editor {
                                policy.editor_grants = all;
                            } else {
                                policy.admin_grants = all;
                            }
                            out.send.push(Command::Policy { room: room.id.clone(), policy });
                        }
                    });
            }
        }

        // what you can edit, and whether the layer you're on is one of them
        heading(ui, pal, "Your access", "");
        card(ui, pal, |ui| {
            if let Some(my) = &my {
                let reach = reach(project, &room.policy, my);
                ui.label(RichText::new(reach_text(my, &reach)).strong());
                let q0rg = app.session.current_q0rg_id;
                let layer = app.session.current_layer_id;
                if let Some(name) = layer_name(project, q0rg, layer) {
                    let access = layer_access(project, &room.policy, my, q0rg, layer);
                    let (text, color) = if access.allowed() {
                        (format!("✓ You can edit the current layer “{name}”"), pal.ok)
                    } else if my.rank == Rank::Viewer {
                        (format!("Viewing “{name}”"), pal.dim)
                    } else {
                        (
                            format!("The current layer “{name}” is read-only for you"),
                            pal.warn,
                        )
                    };
                    ui.label(RichText::new(text).small().color(color));
                }
                access_chips(ui, pal, project, &room.policy, my);
                if my.rank == Rank::Viewer {
                    ui.label(
                        RichText::new("Ask the owner for editor access to work on layers.")
                            .small()
                            .color(pal.dim),
                    );
                } else if !reach.whole && reach.editable > 0 {
                    egui::CollapsingHeader::new(RichText::new("Layers you can edit").small())
                        .id_source("online_my_layers")
                        .show(ui, |ui| {
                            for q0rg in &project.q0rgs {
                                let names: Vec<String> = timeline_rows(project, q0rg)
                                    .into_iter()
                                    .filter(|row| {
                                        layer_access(project, &room.policy, my, q0rg.q0rg_id, row.0)
                                            .allowed()
                                    })
                                    .map(|row| if row.2 { format!("{}/", row.1) } else { row.1 })
                                    .collect();
                                if names.is_empty() {
                                    continue;
                                }
                                ui.label(RichText::new(&q0rg.name).small().strong());
                                ui.label(RichText::new(names.join(", ")).small().color(pal.dim));
                            }
                        });
                }
            } else {
                ui.label(RichText::new("Your membership is still loading.").color(pal.dim));
            }
        });

        // participants
        let mut members = room.members.clone();
        members.sort_by(|a, b| {
            a.banned.cmp(&b.banned).then(b.rank.cmp(&a.rank)).then(
                name_of(a.user_id)
                    .username
                    .cmp(&name_of(b.user_id).username),
            )
        });
        let here = room.presence.len();
        heading(
            ui,
            pal,
            &format!("People · {}", members.iter().filter(|m| !m.banned).count()),
            &if here > 0 {
                format!("{} here now", plural(here, "person", "people"))
            } else {
                String::new()
            },
        );
        for member in &members {
            let person = name_of(member.user_id);
            let is_me = Some(member.user_id) == me;
            let presence = room.presence.iter().find(|p| p.user_id == member.user_id);
            let expanded = view.expanded == Some(member.user_id);
            let frame_fill = if expanded {
                pal.card
            } else {
                Color32::TRANSPARENT
            };
            egui::Frame::none()
                .fill(frame_fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if expanded {
                        pal.line
                    } else {
                        Color32::TRANSPARENT
                    },
                ))
                .rounding(6.0)
                .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        avatar(ui, avatars, &person, ROW_AVATAR, out);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let toggle = ui
                                .selectable_label(expanded, if expanded { "▴" } else { "▾" })
                                .on_hover_text(if owner && !is_me && member.rank != Rank::Owner {
                                    "Access and moderation"
                                } else {
                                    "Details"
                                });
                            if toggle.clicked() {
                                view.expanded = if expanded { None } else { Some(member.user_id) };
                                view.member_draft = None;
                                view.filter.clear();
                            }
                            if let (Some(presence), false) = (presence, is_me) {
                                if let (Some(q0rg_id), Some(frame)) =
                                    (presence.presence.q0rg_id, presence.presence.frame)
                                {
                                    if ui
                                        .small_button("Go to")
                                        .on_hover_text("Show the frame they're on")
                                        .clicked()
                                    {
                                        out.follow = Some(Follow {
                                            q0rg_id,
                                            frame,
                                            navigated: false,
                                        });
                                    }
                                }
                            }
                            rank_chip(ui, pal, member.rank);
                            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.horizontal(|ui| {
                                    if presence.is_some() {
                                        dot(ui, pal.ok, true).on_hover_text("In the room now");
                                    }
                                    let mut name = RichText::new(&person.username).strong();
                                    if member.banned {
                                        name = name.strikethrough().color(pal.dim);
                                    }
                                    ui.add(egui::Label::new(name).truncate(true));
                                    if is_me {
                                        ui.label(RichText::new("you").small().color(pal.dim));
                                    }
                                });
                                let detail = if member.banned {
                                    "banned".to_string()
                                } else if let Some(p) = presence {
                                    let mut bits = Vec::new();
                                    if let (Some(q), Some(l)) =
                                        (p.presence.q0rg_id, p.presence.layer_id)
                                    {
                                        if let Some(layer) = layer_name(project, q, l) {
                                            bits.push(format!("on {layer}"));
                                        }
                                    }
                                    if let Some(frame) = p.presence.frame {
                                        bits.push(format!("frame {}", u32::from(frame) + 1));
                                    }
                                    if project.q0rgs.len() > 1 {
                                        if let Some(q) = p.presence.q0rg_id {
                                            bits.push(q0rg_name(project, q));
                                        }
                                    }
                                    if bits.is_empty() {
                                        "here now".into()
                                    } else {
                                        bits.join(" · ")
                                    }
                                } else {
                                    member_summary(project, &room.policy, member)
                                };
                                ui.add(
                                    egui::Label::new(RichText::new(detail).small().color(pal.dim))
                                        .truncate(true),
                                );
                            });
                        });
                    });
                    if expanded {
                        member_detail(ui, pal, view, busy, room, member, my_rank, is_me, out);
                    }
                });
        }

        // invitations
        let can_invite = my.as_ref().is_some_and(|m| {
            !m.banned && (m.rank >= Rank::Admin || room.policy.members_can_invite)
        });
        if can_invite {
            heading(
                ui,
                pal,
                "Invite friends",
                if owner {
                    "Choose what they join as. You can change it any time."
                } else {
                    "People you invite join as viewers."
                },
            );
            if owner {
                ui.horizontal(|ui| {
                    ui.label("Join as");
                    egui::ComboBox::from_id_source("online_invite_rank")
                        .selected_text(rank_text(view.invite_rank))
                        .show_ui(ui, |ui| {
                            for rank in [Rank::Viewer, Rank::Editor, Rank::Admin] {
                                ui.selectable_value(&mut view.invite_rank, rank, rank_text(rank))
                                    .on_hover_text(rank_help(rank));
                            }
                        });
                });
            } else {
                view.invite_rank = Rank::Viewer;
            }
            let invited: Vec<i64> = room.invites.iter().map(|i| i.invitee_id).collect();
            let eligible: Vec<&FriendRow> = friends
                .iter()
                .filter(|f| {
                    f.status == "accepted" && !room.members.iter().any(|m| m.user_id == f.person.id)
                })
                .collect();
            if eligible.is_empty() {
                ui.label(
                    RichText::new(if friends.iter().any(|f| f.status == "accepted") {
                        "All your friends are already in this room."
                    } else {
                        "Invitations go to friends. Add some in the Friends tab."
                    })
                    .small()
                    .color(pal.dim),
                );
            }
            for friend in eligible {
                let id = friend.person.id;
                let inviting = busy.contains(&format!("invite:{id}"));
                ui.horizontal(|ui| {
                    avatar(ui, avatars, &friend.person, 22.0, out);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if invited.contains(&id) {
                            "Invite again"
                        } else {
                            "Invite"
                        };
                        if ui
                            .add_enabled(!inviting, egui::Button::new(label))
                            .on_hover_text(format!("Invite as {}", rank_text(view.invite_rank)))
                            .clicked()
                        {
                            out.send.push(Command::Invite {
                                room: room.id.clone(),
                                user_id: id,
                                rank: view.invite_rank,
                            });
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(&friend.person.username).truncate(true));
                            if invited.contains(&id) {
                                ui.label(RichText::new("invited").small().color(pal.dim));
                            }
                        });
                    });
                });
            }
        }
        if !room.invites.is_empty() {
            heading(
                ui,
                pal,
                &format!("Waiting for an answer · {}", room.invites.len()),
                "",
            );
            for invite in &room.invites {
                let revoking = busy.contains(&format!("revoke:{}", invite.id));
                let person = name_of(invite.invitee_id);
                let mine = Some(invite.inviter_id) == me;
                ui.horizontal(|ui| {
                    avatar(ui, avatars, &person, 22.0, out);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if (owner || mine)
                            && ui
                                .add_enabled(!revoking, egui::Button::new("Withdraw"))
                                .on_hover_text("Cancel this invitation")
                                .clicked()
                        {
                            out.send.push(Command::RevokeInvite {
                                room: room.id.clone(),
                                id: invite.id.clone(),
                            });
                        }
                        rank_chip(ui, pal, invite.rank);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(&invite.invitee_name).truncate(true));
                            let by = if mine {
                                String::new()
                            } else {
                                format!(" · by {}", invite.inviter_name)
                            };
                            ui.label(
                                RichText::new(format!("{}{by}", until(now, invite.expires_at)))
                                    .small()
                                    .color(pal.dim),
                            );
                        });
                    });
                });
            }
        }

        if owner {
            room_settings(ui, pal, view, busy, room, out);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn member_detail(
    ui: &mut Ui,
    pal: &Palette,
    view: &mut View,
    busy: &BTreeSet<String>,
    room: &RoomSession,
    member: &Member,
    my_rank: Rank,
    is_me: bool,
    out: &mut Out,
) {
    let project = &room.confirmed;
    ui.add_space(2.0);
    let reach = reach(project, &room.policy, member);
    ui.label(RichText::new(reach_text(member, &reach)).small().strong());
    access_chips(ui, pal, project, &room.policy, member);
    let owner_editing = my_rank == Rank::Owner && !is_me && member.rank != Rank::Owner;
    if owner_editing {
        // a draft nobody touched follows the server; an edited one is kept
        let stale = view.member_draft.as_ref().is_none_or(|d| {
            d.user_id != member.user_id
                || (d.base != *member && d.rank == d.base.rank && d.grants == d.base.grants)
        });
        if stale {
            view.member_draft = Some(MemberDraft {
                user_id: member.user_id,
                rank: member.rank,
                grants: member.grants.clone(),
                base: member.clone(),
            });
        }
        let draft = view.member_draft.as_mut().unwrap();
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Rank");
            egui::ComboBox::from_id_source(("online_member_rank", member.user_id))
                .selected_text(rank_text(draft.rank))
                .show_ui(ui, |ui| {
                    for rank in [Rank::Viewer, Rank::Editor, Rank::Admin] {
                        ui.selectable_value(&mut draft.rank, rank, rank_text(rank))
                            .on_hover_text(rank_help(rank));
                    }
                });
        });
        ui.label(RichText::new(rank_help(draft.rank)).small().color(pal.dim));
        ui.add_space(2.0);
        ui.label(RichText::new("Assigned for editing").strong());
        let preview = Member {
            user_id: member.user_id,
            rank: if draft.rank == Rank::Viewer {
                Rank::Editor
            } else {
                draft.rank
            },
            grants: draft.grants.clone(),
            banned: false,
        };
        grant_picker(
            ui,
            pal,
            &format!("member-{}", member.user_id),
            project,
            &room.policy,
            &preview,
            &mut draft.grants,
            &mut view.filter,
            false,
        );
        if draft.rank == Rank::Viewer && !draft.grants.is_empty() {
            ui.label(
                RichText::new("Viewers never edit. These assignments take effect if you make them an editor or admin.")
                    .small()
                    .color(pal.warn),
            );
        }
        let dirty = draft.rank != member.rank || draft.grants != member.grants;
        let saving = busy.contains(&format!("member:{}", member.user_id));
        ui.horizontal(|ui| {
            if primary(ui, pal, "Save access", dirty && !saving).clicked() {
                out.send.push(Command::Member {
                    room: room.id.clone(),
                    member: Member {
                        user_id: member.user_id,
                        rank: draft.rank,
                        grants: draft.grants.clone(),
                        banned: member.banned,
                    },
                });
            }
            if ui
                .add_enabled(dirty && !saving, egui::Button::new("Revert"))
                .clicked()
            {
                draft.rank = member.rank;
                draft.grants = member.grants.clone();
            }
            if saving {
                ui.spinner();
            } else if dirty {
                ui.label(RichText::new("unsaved").small().color(pal.warn));
            }
        });
    }
    let can_moderate =
        !is_me && member.rank != Rank::Owner && my_rank >= Rank::Admin && my_rank > member.rank;
    if can_moderate {
        ui.separator();
        let banning = busy.contains(&format!("ban:{}", member.user_id));
        ui.horizontal_wrapped(|ui| {
            if member.banned {
                if ui
                    .add_enabled(!banning, egui::Button::new("Unban"))
                    .clicked()
                {
                    out.send.push(Command::Ban {
                        room: room.id.clone(),
                        user_id: member.user_id,
                        banned: false,
                    });
                }
                ui.label(
                    RichText::new("They can open the room again after this.")
                        .small()
                        .color(pal.dim),
                );
            } else {
                if armed_button(
                    ui,
                    view,
                    pal,
                    &format!("ban:{}", member.user_id),
                    "Ban from room",
                    "Ban and disconnect?",
                    !banning,
                ) {
                    out.send.push(Command::Ban {
                        room: room.id.clone(),
                        user_id: member.user_id,
                        banned: true,
                    });
                }
                ui.label(
                    RichText::new("Disconnects them and blocks rejoining.")
                        .small()
                        .color(pal.dim),
                );
            }
        });
    } else if !is_me && my_rank == Rank::Admin && member.rank >= Rank::Admin {
        ui.label(
            RichText::new("Admins can only moderate editors and viewers.")
                .small()
                .color(pal.dim),
        );
    }
}

fn room_settings(
    ui: &mut Ui,
    pal: &Palette,
    view: &mut View,
    busy: &BTreeSet<String>,
    room: &RoomSession,
    out: &mut Out,
) {
    egui::CollapsingHeader::new(RichText::new("Room settings").strong())
        .id_source("online_room_settings")
        .show(ui, |ui| {
            let project = &room.confirmed;
            let untouched = view.policy_draft.is_some() && view.policy_draft == view.policy_base;
            if view.policy_draft.is_none()
                || (untouched && view.policy_base.as_ref() != Some(&room.policy))
            {
                view.policy_draft = Some(room.policy.clone());
                view.policy_base = Some(room.policy.clone());
            }
            let draft = view.policy_draft.as_mut().unwrap();
            ui.label(
                RichText::new("Rank defaults apply to everyone with that rank, on top of their own assignments.")
                    .small()
                    .color(pal.dim),
            );
            for (index, (rank, title, detail)) in [
                (Rank::Editor, "Editors can edit", "Pick layers per person, or give every editor the same parts below."),
                (Rank::Admin, "Admins can edit", "Moderation is separate: an admin who edits nothing can still ban."),
            ]
            .into_iter()
            .enumerate()
            {
                let list = if rank == Rank::Editor { &mut draft.editor_grants } else { &mut draft.admin_grants };
                let everything = list.iter().any(|g| g.scope == Scope::Project);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(title).strong());
                    if ui
                        .selectable_label(everything, "everything")
                        .on_hover_text("They can draw anywhere in the project")
                        .clicked()
                    {
                        *list = vec![Grant { scope: Scope::Project }];
                    }
                    if ui
                        .selectable_label(!everything, "only what you assign")
                        .on_hover_text("Give each person layers or folders; nothing else is editable")
                        .clicked()
                        && everything
                    {
                        list.clear();
                    }
                });
                if everything {
                    continue;
                }
                egui::CollapsingHeader::new(format!(
                    "Same for every {}: {}",
                    rank_text(rank),
                    if list.is_empty() { "nothing".to_string() } else { plural(list.len(), "item", "items") }
                ))
                .id_source(("online_defaults", rank_text(rank)))
                .show(ui, |ui| {
                    ui.label(RichText::new(detail).small().color(pal.dim));
                    let preview_policy = RoomPolicy {
                        editor_grants: list.clone(),
                        admin_grants: list.clone(),
                        ..RoomPolicy::default()
                    };
                    let preview = Member { user_id: 0, rank, grants: Vec::new(), banned: false };
                    grant_picker(
                        ui,
                        pal,
                        &format!("defaults-{}", rank_text(rank)),
                        project,
                        &preview_policy,
                        &preview,
                        list,
                        &mut view.defaults_filter[index],
                        true,
                    );
                });
            }
            ui.checkbox(&mut draft.members_can_invite, "Members may invite their friends (as viewers)");
            ui.horizontal(|ui| {
                ui.label("Who may save a copy");
                let label = |rank: Rank| match rank {
                    Rank::Viewer => "everyone",
                    Rank::Editor => "editors and up",
                    Rank::Admin => "admins and you",
                    Rank::Owner => "only you",
                };
                egui::ComboBox::from_id_source("room_save_rank")
                    .selected_text(label(draft.save_rank))
                    .show_ui(ui, |ui| {
                        for rank in [Rank::Viewer, Rank::Editor, Rank::Admin, Rank::Owner] {
                            ui.selectable_value(&mut draft.save_rank, rank, label(rank));
                        }
                    })
                    .response
                    .on_hover_text(
                        "Saving, exporting and copying layers or frames out of the room. \
                         Everyone else can look and draw where they're allowed, but not keep the project.",
                    );
            });
            ui.horizontal(|ui| {
                ui.label("Member limit");
                let current = room.members.len().max(1);
                ui.add(egui::DragValue::new(&mut draft.max_members).clamp_range(current..=256));
            });
            let dirty = *draft != room.policy;
            let saving = busy.contains("policy");
            ui.horizontal(|ui| {
                if primary(ui, pal, "Save settings", dirty && !saving).clicked() {
                    out.send.push(Command::Policy { room: room.id.clone(), policy: draft.clone() });
                }
                if ui.add_enabled(dirty && !saving, egui::Button::new("Discard")).clicked() {
                    *draft = room.policy.clone();
                }
                if saving {
                    ui.spinner();
                } else if dirty {
                    ui.label(RichText::new("unsaved").small().color(pal.warn));
                }
            });
            ui.separator();
            ui.label(
                RichText::new("Closing disconnects everyone. The project and its history stay saved; you can reopen it later.")
                    .small()
                    .color(pal.dim),
            );
            if armed_button(ui, view, pal, "close-room", "Close room", "Close it for everyone?", !busy.contains("close")) {
                out.send.push(Command::CloseRoom { room: room.id.clone(), closed: true });
            }
            if armed_button(ui,view,pal,"delete-room","Delete room","Permanently delete its project and history?",!busy.contains("delete-room")) {
                out.send.push(Command::DeleteRoom {room:room.id.clone()});
            }
        });
}
