//! Animate timeline masks use filled geometry, ignoring its color and alpha.
//! Keep that conversion local to imported mask sources: native q0s masks can
//! still use alpha, and symbols shared with ordinary artwork keep their look.
use std::collections::HashMap;

use q0s_format::v2::{Asset, LayerKey, LayerKind, Placement, PlacementFx, ProjectV2, Target};

use crate::{ImportError, ImportedProject};

pub(crate) fn normalize(imported: &mut ImportedProject) -> Result<(), ImportError> {
    let masks: Vec<_> = imported
        .project
        .q0rgs
        .iter()
        .flat_map(|q| {
            q.layers
                .iter()
                .filter(|layer| {
                    imported
                        .project
                        .layer_metadata(q.q0rg_id, layer.layer_id)
                        .kind
                        == LayerKind::Mask
                })
                .map(|layer| (q.q0rg_id, layer.layer_id, layer.placements.clone()))
        })
        .collect();
    if masks.is_empty() {
        return Ok(());
    }
    let project = &mut imported.project;
    let mut sources = Sources {
        assets: project
            .assets
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id(), i))
            .collect(),
        symbols: project
            .q0rgs
            .iter()
            .enumerate()
            .map(|(i, q)| (q.q0rg_id, i))
            .collect(),
        next_asset: project
            .assets
            .iter()
            .map(|a| u32::from(a.id()))
            .max()
            .unwrap_or(0)
            + 1,
        next_symbol: project
            .q0rgs
            .iter()
            .map(|q| u32::from(q.q0rg_id))
            .max()
            .unwrap_or(0)
            + 1,
        copied_assets: HashMap::new(),
        copied_symbols: HashMap::new(),
    };
    for (q0rg_id, layer_id, mut placements) in masks {
        for placement in &mut placements {
            sources.placement(project, placement, 0)?;
        }
        let index = sources.symbols[&q0rg_id];
        let layer = project.q0rgs[index]
            .layers
            .iter_mut()
            .find(|l| l.layer_id == layer_id)
            .unwrap();
        layer.placements = placements;
    }
    // Missing-font replacement must also update the mask's private outline.
    let pending = imported.pending_text.clone();
    for mut text in pending {
        if let Some(&copy) = sources.copied_assets.get(&text.asset_id) {
            if copy != text.asset_id {
                text.asset_id = copy;
                imported.pending_text.push(text);
            }
        }
    }
    imported.report.imported_layers = project.q0rgs.iter().map(|q| q.layers.len()).sum();
    imported.report.imported_keyframes = project
        .q0rgs
        .iter()
        .flat_map(|q| &q.layers)
        .map(|l| l.keyframe_frames().len())
        .sum();
    q0s_format::v2::validate(project).map_err(|error| {
        ImportError::Invalid(format!("converted mask geometry is invalid: {error}"))
    })
}

struct Sources {
    assets: HashMap<u16, usize>,
    symbols: HashMap<u16, usize>,
    next_asset: u32,
    next_symbol: u32,
    copied_assets: HashMap<u16, u16>,
    copied_symbols: HashMap<u16, u16>,
}

impl Sources {
    fn placement(
        &mut self,
        project: &mut ProjectV2,
        placement: &mut Placement,
        depth: usize,
    ) -> Result<(), ImportError> {
        // Keep playback ranges and geometry/tweens; visual effects do not
        // attenuate or blur a stencil mask in Animate.
        placement.fx = PlacementFx {
            q0rg_playback: placement.fx.q0rg_playback,
            ..PlacementFx::default()
        };
        placement.target = match placement.target {
            Target::Asset(id) => Target::Asset(self.asset(project, id)?),
            Target::Q0rg(id) => Target::Q0rg(self.symbol(project, id, depth)?),
        };
        Ok(())
    }

    fn asset(&mut self, project: &mut ProjectV2, id: u16) -> Result<u16, ImportError> {
        if let Some(&copy) = self.copied_assets.get(&id) {
            return Ok(copy);
        }
        let Some(&index) = self.assets.get(&id) else {
            return Err(ImportError::Invalid(format!(
                "mask refers to missing asset {id}"
            )));
        };
        let Asset::Vector(vector) = &project.assets[index] else {
            return Ok(id);
        };
        let gradient = project
            .asset_appearances
            .get(&id)
            .is_some_and(|a| a.fill_gradient.is_some());
        if vector.fill.is_none_or(|color| color.a == 255) && vector.stroke.is_none() && !gradient {
            self.copied_assets.insert(id, id);
            return Ok(id);
        }
        let copy = u16::try_from(self.next_asset).map_err(|_| {
            ImportError::Invalid("no asset ids remain for imported mask geometry".into())
        })?;
        self.next_asset += 1;
        let mut vector = vector.clone();
        vector.asset_id = copy;
        if let Some(fill) = &mut vector.fill {
            fill.a = 255;
        }
        vector.stroke = None;
        if let Some(name) = project.asset_names.get(&id).cloned() {
            project.asset_names.insert(copy, name);
        }
        self.assets.insert(copy, project.assets.len());
        project.assets.push(Asset::Vector(vector));
        self.copied_assets.insert(id, copy);
        self.copied_assets.insert(copy, copy);
        Ok(copy)
    }

    fn symbol(
        &mut self,
        project: &mut ProjectV2,
        id: u16,
        depth: usize,
    ) -> Result<u16, ImportError> {
        if let Some(&copy) = self.copied_symbols.get(&id) {
            return Ok(copy);
        }
        if depth >= 64 {
            return Err(ImportError::Invalid(
                "imported mask symbols nest too deeply".into(),
            ));
        }
        let Some(&index) = self.symbols.get(&id) else {
            return Err(ImportError::Invalid(format!(
                "mask refers to missing symbol {id}"
            )));
        };
        let copy = u16::try_from(self.next_symbol).map_err(|_| {
            ImportError::Invalid("no symbol ids remain for imported mask geometry".into())
        })?;
        self.next_symbol += 1;
        let mut symbol = project.q0rgs[index].clone();
        symbol.q0rg_id = copy;
        symbol.name.push_str(" / mask");
        // Register before descending so recursive library references stay
        // bounded and point to the same private mask copy.
        self.copied_symbols.insert(id, copy);
        self.copied_symbols.insert(copy, copy);
        for layer in &mut symbol.layers {
            if let Some(metadata) = project
                .layer_metadata
                .get(&LayerKey::new(id, layer.layer_id))
                .copied()
            {
                project
                    .layer_metadata
                    .insert(LayerKey::new(copy, layer.layer_id), metadata);
            }
            for placement in &mut layer.placements {
                self.placement(project, placement, depth + 1)?;
            }
        }
        self.symbols.insert(copy, project.q0rgs.len());
        project.q0rgs.push(symbol);
        Ok(copy)
    }
}
