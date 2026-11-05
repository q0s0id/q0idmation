//! Layer locks and visibility, set the way Animate and Aseprite set them.
//!
//! * Locking (or hiding) all layers remembers which ones it turned on, and
//!   turning them all back off restores the rest as they were: a layer you
//!   had locked yourself stays locked.
//! * Alt-clicking a layer's eye or lock does the same to every *other*
//!   layer, so one layer can be worked on alone and the rest come back.
//! * Pressing on an eye or lock and dragging down the column sets every
//!   row it passes to the same state, in one undo step.
//! * A folder's lock or eye covers what is in it without changing their
//!   own flags, so opening the folder again restores them.

use std::collections::BTreeSet;

use q0s_format::v2::{LayerKey, LayerMetadata};

use crate::app::EditorApp;
use crate::state::Selection;

/// Which of a layer's two switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerFlag {
    Locked,
    Hidden,
}

impl LayerFlag {
    fn noun(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Hidden => "hidden",
        }
    }

    fn undone(self) -> &'static str {
        match self {
            Self::Locked => "unlocked",
            Self::Hidden => "shown",
        }
    }
}

/// A drag down the eye or lock column, painting one state over each row
/// it crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerFlagDrag {
    pub q0rg_id: u16,
    pub flag: LayerFlag,
    pub on: bool,
}

/// What "all" (or "all others") remembers it turned on: by project tab,
/// symbol, switch and the layer left out (`None` for all layers).
pub type LayerFlagMemoryKey = (u64, u16, LayerFlag, Option<u16>);

impl EditorApp {
    fn layer_exists(&self, q0rg_id: u16, layer_id: u16) -> bool {
        self.state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .is_some_and(|q0rg| q0rg.layers.iter().any(|layer| layer.layer_id == layer_id))
    }

    /// The layer's own switch, without what its folders add. A lock you
    /// hold only for yourself in a room counts.
    pub(crate) fn own_layer_flag(&self, q0rg_id: u16, layer_id: u16, flag: LayerFlag) -> bool {
        let metadata = self.state.project.layer_metadata(q0rg_id, layer_id);
        match flag {
            LayerFlag::Hidden => metadata.hidden,
            LayerFlag::Locked => metadata.locked || self.online.locally_locked(q0rg_id, layer_id),
        }
    }

    /// The switch as it acts: the layer's own, or one of its folders'.
    pub(crate) fn effective_layer_flag(
        &self,
        q0rg_id: u16,
        layer_id: u16,
        flag: LayerFlag,
    ) -> bool {
        match flag {
            LayerFlag::Hidden => !self.state.project.layer_is_visible(q0rg_id, layer_id),
            LayerFlag::Locked => {
                self.state.project.layer_is_locked(q0rg_id, layer_id)
                    || self.online.locally_locked(q0rg_id, layer_id)
            }
        }
    }

    /// Set the layer's own switch. In a room a new lock is yours alone,
    /// as one clicked on is.
    fn set_own_layer_flag(&mut self, q0rg_id: u16, layer_id: u16, flag: LayerFlag, on: bool) {
        if self.own_layer_flag(q0rg_id, layer_id, flag) == on {
            return;
        }
        let tab = self.active_project_tab_id();
        let mut metadata: LayerMetadata = self.state.project.layer_metadata(q0rg_id, layer_id);
        match flag {
            LayerFlag::Hidden => metadata.hidden = on,
            LayerFlag::Locked => {
                if on {
                    if self.online.set_local_lock(tab, q0rg_id, layer_id, true) {
                        self.follow_mask_lock(q0rg_id, layer_id, true);
                        return;
                    }
                    metadata.locked = true;
                } else {
                    self.online.set_local_lock(tab, q0rg_id, layer_id, false);
                    metadata.locked = false;
                }
                self.follow_mask_lock(q0rg_id, layer_id, on);
            }
        }
        self.state
            .project
            .layer_metadata
            .insert(LayerKey::new(q0rg_id, layer_id), metadata);
        self.state.mark_dirty();
    }

    /// Layers of `q0rg_id` that hold artwork (not folders), but `except`.
    fn drawable_layers(&self, q0rg_id: u16, except: Option<u16>) -> Vec<u16> {
        self.state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .map(|q0rg| {
                q0rg.layers
                    .iter()
                    .map(|layer| layer.layer_id)
                    .filter(|id| Some(*id) != except)
                    .filter(|id| !self.state.project.layer_is_folder(q0rg_id, *id))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn layer_flag_memory_key(
        &self,
        q0rg_id: u16,
        flag: LayerFlag,
        except: Option<u16>,
    ) -> LayerFlagMemoryKey {
        (
            self.active_project_tab_id().unwrap_or(0),
            q0rg_id,
            flag,
            except,
        )
    }

    /// Turn `flag` on for every layer (but `except`) when any is off, or,
    /// when all are on, turn off the ones this turned on, so the rest keep
    /// the state they had before. Without such a memory all go off.
    pub(crate) fn toggle_all_layer_flags(
        &mut self,
        q0rg_id: u16,
        flag: LayerFlag,
        except: Option<u16>,
    ) {
        let layers = self.drawable_layers(q0rg_id, except);
        if layers.is_empty() {
            return;
        }
        let key = self.layer_flag_memory_key(q0rg_id, flag, except);
        let turn_on = layers
            .iter()
            .any(|id| !self.effective_layer_flag(q0rg_id, *id, flag));
        self.history
            .snapshot_with_session(&self.state.project, &self.session);
        let others = if except.is_some() { "other " } else { "" };
        if turn_on {
            let mut turned = self
                .session
                .layer_flag_memory
                .remove(&key)
                .unwrap_or_default();
            for layer_id in layers {
                if !self.effective_layer_flag(q0rg_id, layer_id, flag) {
                    self.set_own_layer_flag(q0rg_id, layer_id, flag, true);
                    turned.insert(layer_id);
                }
            }
            self.session.layer_flag_memory.insert(key, turned);
            self.session.status = format!("all {others}layers {}", flag.noun());
        } else {
            let remembered: BTreeSet<u16> = self
                .session
                .layer_flag_memory
                .remove(&key)
                .unwrap_or_default()
                .into_iter()
                .filter(|id| self.layer_exists(q0rg_id, *id))
                .collect();
            if remembered.is_empty() {
                // Nothing to go back to: everything goes off, folders too.
                self.clear_layer_flags(q0rg_id, flag, except);
                self.session.status = format!("all {others}layers {}", flag.undone());
            } else {
                for layer_id in &remembered {
                    self.set_own_layer_flag(q0rg_id, *layer_id, flag, false);
                }
                self.session.status = format!(
                    "{} {others}layer(s) {}; the rest stay as they were",
                    remembered.len(),
                    flag.undone()
                );
            }
        }
        self.session.selection = Selection::None;
    }

    fn clear_layer_flags(&mut self, q0rg_id: u16, flag: LayerFlag, except: Option<u16>) {
        let layers: Vec<u16> = self
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .map(|q0rg| q0rg.layers.iter().map(|layer| layer.layer_id).collect())
            .unwrap_or_default();
        for layer_id in layers.into_iter().filter(|id| Some(*id) != except) {
            self.set_own_layer_flag(q0rg_id, layer_id, flag, false);
        }
    }

    /// Turn `flag` off on every layer and folder, forgetting what "all"
    /// remembered.
    pub(crate) fn clear_all_layer_flags(&mut self, q0rg_id: u16, flag: LayerFlag) {
        if !self
            .state
            .project
            .q0rgs
            .iter()
            .any(|q0rg| q0rg.q0rg_id == q0rg_id)
        {
            return;
        }
        self.history
            .snapshot_with_session(&self.state.project, &self.session);
        self.session
            .layer_flag_memory
            .retain(|(_, q0rg, kind, _), _| *q0rg != q0rg_id || *kind != flag);
        self.clear_layer_flags(q0rg_id, flag, None);
        self.session.selection = Selection::None;
        self.session.status = format!("every layer {}", flag.undone());
    }

    /// One row of a drag down the eye or lock column. `first` is the row
    /// the drag started on and takes the drag's one undo step.
    pub(crate) fn paint_layer_flag(
        &mut self,
        q0rg_id: u16,
        layer_id: u16,
        flag: LayerFlag,
        on: bool,
        first: bool,
    ) {
        if !self.layer_exists(q0rg_id, layer_id) {
            return;
        }
        if first {
            self.history
                .snapshot_with_session(&self.state.project, &self.session);
        }
        if self.own_layer_flag(q0rg_id, layer_id, flag) == on {
            return;
        }
        self.set_own_layer_flag(q0rg_id, layer_id, flag, on);
        self.session.selection = Selection::None;
        self.session.status = format!(
            "layers {} as you drag",
            if on { flag.noun() } else { flag.undone() }
        );
    }

    pub(crate) fn layer_name(&self, q0rg_id: u16, layer_id: u16) -> String {
        self.state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .and_then(|q0rg| q0rg.layers.iter().find(|layer| layer.layer_id == layer_id))
            .map(|layer| layer.name.clone())
            .unwrap_or_default()
    }

    /// The folder holding `layer_id` whose own switch covers it, if any.
    pub(crate) fn folder_setting_layer_flag(
        &self,
        q0rg_id: u16,
        layer_id: u16,
        flag: LayerFlag,
    ) -> Option<u16> {
        let mut parent = self.state.project.layer_parent_folder(q0rg_id, layer_id);
        let mut steps = 0;
        while let Some(folder) = parent {
            if self.own_layer_flag(q0rg_id, folder, flag) {
                return Some(folder);
            }
            steps += 1;
            if steps > 64 {
                break;
            }
            parent = self.state.project.layer_parent_folder(q0rg_id, folder);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use egui::Context;

    /// A symbol with three layers, all unlocked and shown.
    fn three_layers() -> (EditorApp, u16, Vec<u16>) {
        let mut app = EditorApp::default();
        let ctx = Context::default();
        app.handle(&ctx, Action::AddLayer);
        app.handle(&ctx, Action::AddLayer);
        let q0rg_id = app.session.current_q0rg_id;
        let layers = app
            .state
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == q0rg_id)
            .unwrap()
            .layers
            .iter()
            .map(|layer| layer.layer_id)
            .collect::<Vec<_>>();
        assert_eq!(layers.len(), 3);
        (app, q0rg_id, layers)
    }

    fn locked(app: &EditorApp, q0rg_id: u16, layer_id: u16) -> bool {
        app.own_layer_flag(q0rg_id, layer_id, LayerFlag::Locked)
    }

    #[test]
    fn unlock_all_keeps_the_layer_you_had_locked() {
        let (mut app, q0rg, layers) = three_layers();
        let ctx = Context::default();
        app.handle(&ctx, Action::ToggleLayerLock(q0rg, layers[1]));
        app.handle(&ctx, Action::ToggleAllLayerLocks(q0rg));
        assert!(layers.iter().all(|id| locked(&app, q0rg, *id)));
        app.handle(&ctx, Action::ToggleAllLayerLocks(q0rg));
        assert!(!locked(&app, q0rg, layers[0]));
        assert!(
            locked(&app, q0rg, layers[1]),
            "it was locked before Lock all"
        );
        assert!(!locked(&app, q0rg, layers[2]));
        // Shift-click is the way to unlock everything.
        app.handle(&ctx, Action::UnlockAllLayers(q0rg));
        assert!(layers.iter().all(|id| !locked(&app, q0rg, *id)));
    }

    #[test]
    fn hide_all_and_back_restores_what_was_hidden() {
        let (mut app, q0rg, layers) = three_layers();
        let ctx = Context::default();
        app.handle(&ctx, Action::ToggleLayerVisibility(q0rg, layers[2]));
        app.handle(&ctx, Action::ToggleAllLayerVisibility(q0rg));
        app.handle(&ctx, Action::ToggleAllLayerVisibility(q0rg));
        let hidden = |id| app.own_layer_flag(q0rg, id, LayerFlag::Hidden);
        assert!(!hidden(layers[0]) && !hidden(layers[1]));
        assert!(hidden(layers[2]));
    }

    #[test]
    fn alt_click_locks_the_others_and_gives_them_back() {
        let (mut app, q0rg, layers) = three_layers();
        let ctx = Context::default();
        app.handle(&ctx, Action::ToggleLayerLock(q0rg, layers[2]));
        app.handle(&ctx, Action::ToggleOtherLayerLocks(q0rg, layers[0]));
        assert!(
            !locked(&app, q0rg, layers[0]),
            "the one worked on stays open"
        );
        assert!(locked(&app, q0rg, layers[1]) && locked(&app, q0rg, layers[2]));
        app.handle(&ctx, Action::ToggleOtherLayerLocks(q0rg, layers[0]));
        assert!(!locked(&app, q0rg, layers[1]));
        assert!(locked(&app, q0rg, layers[2]));
    }

    #[test]
    fn dragging_down_the_lock_column_is_one_undo_step() {
        let (mut app, q0rg, layers) = three_layers();
        let ctx = Context::default();
        app.handle(
            &ctx,
            Action::PaintLayerFlag(q0rg, layers[0], LayerFlag::Locked, true, true),
        );
        app.handle(
            &ctx,
            Action::PaintLayerFlag(q0rg, layers[1], LayerFlag::Locked, true, false),
        );
        app.handle(
            &ctx,
            Action::PaintLayerFlag(q0rg, layers[2], LayerFlag::Locked, true, false),
        );
        assert!(layers.iter().all(|id| locked(&app, q0rg, *id)));
        app.handle(&ctx, Action::Undo);
        assert!(layers.iter().all(|id| !locked(&app, q0rg, *id)));
    }

    #[test]
    fn a_layer_locked_by_its_folder_says_so() {
        let (mut app, q0rg, layers) = three_layers();
        let ctx = Context::default();
        app.handle(&ctx, Action::AddLayerFolder);
        let folder = app.session.current_layer_id;
        let mut metadata = app.state.project.layer_metadata(q0rg, layers[0]);
        metadata.parent_folder_id = Some(folder);
        app.state
            .project
            .layer_metadata
            .insert(LayerKey::new(q0rg, layers[0]), metadata);
        app.handle(&ctx, Action::ToggleLayerLock(q0rg, folder));
        assert!(app.effective_layer_flag(q0rg, layers[0], LayerFlag::Locked));
        app.handle(&ctx, Action::ToggleLayerLock(q0rg, layers[0]));
        assert!(
            !locked(&app, q0rg, layers[0]),
            "its own switch is left alone"
        );
        assert!(
            app.session.status.contains("folder"),
            "{}",
            app.session.status
        );
    }
}
