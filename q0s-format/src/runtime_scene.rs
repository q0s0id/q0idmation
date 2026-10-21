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
    /// Radians; live physics turns bodies with it.
    pub rotation: Option<f32>,
}

impl PlacementTransformOverride {
    pub fn apply_to_transform(self, mut transform: Transform2D) -> Transform2D {
        if let Some(tx) = self.tx.filter(|value| value.is_finite()) {
            transform.tx = tx;
        }
        if let Some(ty) = self.ty.filter(|value| value.is_finite()) {
            transform.ty = ty;
        }
        if let Some(rotation) = self.rotation.filter(|value| value.is_finite()) {
            transform.rotation = rotation;
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
        if let Some(rotation) = self.rotation.filter(|value| value.is_finite()) {
            // Turn the whole linear part, so scale and skew ride along.
            let turn = rotation - affine.a21.atan2(affine.a11);
            let (sin, cos) = turn.sin_cos();
            let (a11, a12, a21, a22) = (affine.a11, affine.a12, affine.a21, affine.a22);
            affine.a11 = cos * a11 - sin * a21;
            affine.a12 = cos * a12 - sin * a22;
            affine.a21 = sin * a11 + cos * a21;
            affine.a22 = sin * a12 + cos * a22;
        }
        affine
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuntimeSceneState {
    placement_transforms: HashMap<InstanceKey, PlacementTransformOverride>,
    children: HashMap<InstanceKey, RuntimeChildState>,
    playback_requests: HashMap<InstanceKey, PlaybackRequest>,
    pub rig_overrides: crate::rig::RigRuntimeOverrides,
    /// Lights as scripts have changed them; `None` lights as authored.
    pub lighting: Option<crate::lighting::LightingData>,
}

/// State below one *placement*, rather than below a shared symbol definition.
/// Two copies of a q0rg therefore have independent animation and transforms.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuntimeChildState {
    pub frame: u16,
    pub scene: RuntimeSceneState,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlaybackRequest {
    pub frame: Option<u16>,
    pub playing: Option<bool>,
}

impl RuntimeSceneState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn child_context(&self, key: InstanceKey, fallback_frame: u16) -> (u16, &Self) {
        self.children
            .get(&key)
            .map_or((fallback_frame, self), |child| (child.frame, &child.scene))
    }

    pub fn take_child(&mut self, key: InstanceKey) -> RuntimeChildState {
        self.children.remove(&key).unwrap_or_default()
    }

    pub fn set_child(&mut self, key: InstanceKey, child: RuntimeChildState) {
        self.children.insert(key, child);
    }

    pub fn request_playback(&mut self, key: InstanceKey, request: PlaybackRequest) {
        if request.frame.is_some() || request.playing.is_some() {
            self.playback_requests.insert(key, request);
        }
    }

    pub fn take_playback_request(&mut self, key: InstanceKey) -> PlaybackRequest {
        self.playback_requests.remove(&key).unwrap_or_default()
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
            rotation: None,
        };
        if self.placement_transforms.get(&key).copied() == Some(next) {
            return false;
        }
        self.placement_transforms.insert(key, next);
        true
    }

    /// Places an object outright: live physics moves and turns its bodies
    /// with this every step.
    pub fn set_transform(&mut self, key: InstanceKey, x: f32, y: f32, rotation: f32) -> bool {
        if !x.is_finite() || !y.is_finite() || !rotation.is_finite() || key.instance_id == 0 {
            return false;
        }
        let next = PlacementTransformOverride {
            tx: Some(x),
            ty: Some(y),
            rotation: Some(rotation),
        };
        if self.placement_transforms.get(&key).copied() == Some(next) {
            return false;
        }
        self.placement_transforms.insert(key, next);
        true
    }

    /// Hands an object's placement back to the timeline.
    pub fn clear_transform(&mut self, key: InstanceKey) -> bool {
        self.placement_transforms.remove(&key).is_some()
    }

    pub fn clear_instance(&mut self, key: InstanceKey) -> bool {
        let removed = self.placement_transforms.remove(&key).is_some();
        let request = self.playback_requests.remove(&key).is_some();
        self.children.remove(&key).is_some() || removed || request
    }

    pub fn is_empty(&self) -> bool {
        self.placement_transforms.is_empty()
            && self.children.is_empty()
            && self.rig_overrides.is_empty()
            && self.lighting.is_none()
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
    fn a_turned_override_turns_the_affine_like_the_transform() {
        let key = InstanceKey::new(1, 7);
        let authored = Transform2D {
            tx: 10.0,
            ty: 20.0,
            sx: 2.0,
            sy: 3.0,
            rotation: 0.4,
            skew_x: 0.0,
            skew_y: 0.0,
        };
        let mut state = RuntimeSceneState::new();
        assert!(state.set_transform(key, 30.0, 40.0, 1.3));
        let by_transform = Affine::from_transform(state.effective_transform(key, authored));
        let by_affine = state.effective_affine(key, Affine::from_transform(authored));
        for (a, b) in [
            (by_transform.a11, by_affine.a11),
            (by_transform.a12, by_affine.a12),
            (by_transform.a21, by_affine.a21),
            (by_transform.a22, by_affine.a22),
            (by_transform.tx, by_affine.tx),
            (by_transform.ty, by_affine.ty),
        ] {
            assert!((a - b).abs() < 1.0e-5, "{by_transform:?} vs {by_affine:?}");
        }
        assert!(state.clear_transform(key));
        assert!(state.is_empty());
    }

    #[test]
    fn invalid_or_idless_position_never_enters_runtime_state() {
        let mut state = RuntimeSceneState::new();
        assert!(!state.set_position(InstanceKey::new(1, 0), 1.0, 2.0));
        assert!(!state.set_position(InstanceKey::new(1, 2), f32::NAN, 2.0));
        assert!(state.is_empty());
    }
}
