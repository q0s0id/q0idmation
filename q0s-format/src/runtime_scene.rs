//! Ephemeral runtime scene overrides shared by q0player and renderers.
//!
//! This state is deliberately outside `ProjectV2`: q0lang can move an authored
//! display object at runtime without mutating the saved movie/project model.

use std::collections::HashMap;

use crate::transform::Affine;
use crate::v2::{InstanceKey, Transform2D};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlacementTransformOverride {
    pub tx: Option<f32>,
    pub ty: Option<f32>,
}

impl PlacementTransformOverride {
    pub fn apply_to_transform(self, mut transform: Transform2D) -> Transform2D {
        if let Some(tx) = self.tx.filter(|value| value.is_finite()) {
            transform.tx = tx;
        }
        if let Some(ty) = self.ty.filter(|value| value.is_finite()) {
            transform.ty = ty;
        }
        transform
    }

    pub fn apply_to_affine(self, mut affine: Affine) -> Affine {
        if let Some(tx) = self.tx.filter(|value| value.is_finite()) {
            affine.tx = tx;
        }
        if let Some(ty) = self.ty.filter(|value| value.is_finite()) {
            affine.ty = ty;
        }
        affine
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuntimeSceneState {
    placement_transforms: HashMap<InstanceKey, PlacementTransformOverride>,
}

impl RuntimeSceneState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn transform_override(&self, key: InstanceKey) -> Option<PlacementTransformOverride> {
        self.placement_transforms.get(&key).copied()
    }

    pub fn effective_transform(&self, key: InstanceKey, authored: Transform2D) -> Transform2D {
        self.transform_override(key)
            .map_or(authored, |override_| override_.apply_to_transform(authored))
    }

    pub fn effective_affine(&self, key: InstanceKey, authored: Affine) -> Affine {
        self.transform_override(key)
            .map_or(authored, |override_| override_.apply_to_affine(authored))
    }

    pub fn set_position(&mut self, key: InstanceKey, x: f32, y: f32) -> bool {
        if !x.is_finite() || !y.is_finite() || key.instance_id == 0 {
            return false;
        }
        let next = PlacementTransformOverride {
            tx: Some(x),
            ty: Some(y),
        };
        if self.placement_transforms.get(&key).copied() == Some(next) {
            return false;
        }
        self.placement_transforms.insert(key, next);
        true
    }

    pub fn clear_instance(&mut self, key: InstanceKey) -> bool {
        self.placement_transforms.remove(&key).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.placement_transforms.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_position_override_changes_only_translation() {
        let key = InstanceKey::new(1, 7);
        let authored = Transform2D {
            tx: 10.0,
            ty: 20.0,
            sx: 2.0,
            sy: 3.0,
            rotation: 0.4,
            skew_x: 0.1,
            skew_y: -0.2,
        };
        let mut state = RuntimeSceneState::new();
        assert!(state.set_position(key, 30.0, 40.0));
        let effective = state.effective_transform(key, authored);
        assert_eq!(effective.tx, 30.0);
        assert_eq!(effective.ty, 40.0);
        assert_eq!(effective.sx, authored.sx);
        assert_eq!(effective.sy, authored.sy);
        assert_eq!(effective.rotation, authored.rotation);
        assert_eq!(effective.skew_x, authored.skew_x);
        assert_eq!(effective.skew_y, authored.skew_y);
    }

    #[test]
    fn invalid_or_idless_position_never_enters_runtime_state() {
        let mut state = RuntimeSceneState::new();
        assert!(!state.set_position(InstanceKey::new(1, 0), 1.0, 2.0));
        assert!(!state.set_position(InstanceKey::new(1, 2), f32::NAN, 2.0));
        assert!(state.is_empty());
    }
}
