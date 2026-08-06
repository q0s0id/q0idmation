//! 2×3 affine matrix shared between the rasteriser, the editor's GPU
//! renderer and the player's GPU renderer.
//!
//! `Transform2D` (in `v2`) stores a logical (sx, sy, rotation, skew_x,
//! skew_y, tx, ty) tuple — fine for the editor's UI controls but the
//! decomposition is *not* closed under composition: composing two
//! sheared+rotated transforms can't be re-expressed as another tuple
//! without losing fidelity.
//!
//! Renderers that walk a q0rg → child q0rg → asset hierarchy therefore
//! work in `Affine` instead — pure matrix multiplication, parent
//! transformations propagate cleanly into children. The only place we
//! convert back to `Transform2D` is when persisting a placement's pose
//! into the data model.
//!
//! Also exposes inverse + uniform_scale (used to adjust stroke width
//! under non-trivial parent transforms).

use crate::v2::{Transform2D, Vec2};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a11: f32,
    pub a12: f32,
    pub a21: f32,
    pub a22: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine {
    pub const IDENTITY: Self = Self {
        a11: 1.0,
        a12: 0.0,
        a21: 0.0,
        a22: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn from_uniform_scale(s: f32) -> Self {
        Self::from_scale(s, s)
    }

    pub fn from_scale(sx: f32, sy: f32) -> Self {
        Self {
            a11: sx,
            a12: 0.0,
            a21: 0.0,
            a22: sy,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Build the matrix that matches `Transform2D::apply` (in legacy
    /// renderer code) byte-for-byte: scale → horizontal shear by
    /// skew_x → vertical shear by skew_y (operating on the already-
    /// sheared x) → rotate → translate.
    pub fn from_transform(t: Transform2D) -> Self {
        let m11 = t.sx;
        let m12 = t.sy * t.skew_x.tan();
        let m21 = t.sx * t.skew_y.tan();
        let m22 = t.sy * (1.0 + t.skew_x.tan() * t.skew_y.tan());
        let (sin, cos) = t.rotation.sin_cos();
        Self {
            a11: cos * m11 - sin * m21,
            a12: cos * m12 - sin * m22,
            a21: sin * m11 + cos * m21,
            a22: sin * m12 + cos * m22,
            tx: t.tx,
            ty: t.ty,
        }
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            self.a11 * p.x + self.a12 * p.y + self.tx,
            self.a21 * p.x + self.a22 * p.y + self.ty,
        )
    }

    /// `result.apply(p)` ≡ `parent.apply(child.apply(p))` — child first,
    /// then parent. This is what makes parent skew / non-uniform scale
    /// correctly propagate into child placements: the children inherit
    /// the parent's full 2×3, not a lossy SRSk-decomposed copy.
    pub fn compose(parent: Self, child: Self) -> Self {
        Self {
            a11: parent.a11 * child.a11 + parent.a12 * child.a21,
            a12: parent.a11 * child.a12 + parent.a12 * child.a22,
            a21: parent.a21 * child.a11 + parent.a22 * child.a21,
            a22: parent.a21 * child.a12 + parent.a22 * child.a22,
            tx: parent.a11 * child.tx + parent.a12 * child.ty + parent.tx,
            ty: parent.a21 * child.tx + parent.a22 * child.ty + parent.ty,
        }
    }

    pub fn inverse(&self) -> Option<Self> {
        let det = self.a11 * self.a22 - self.a12 * self.a21;
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let a11 = self.a22 * inv;
        let a12 = -self.a12 * inv;
        let a21 = -self.a21 * inv;
        let a22 = self.a11 * inv;
        Some(Self {
            a11,
            a12,
            a21,
            a22,
            tx: -(a11 * self.tx + a12 * self.ty),
            ty: -(a21 * self.tx + a22 * self.ty),
        })
    }

    /// √|det| — area scale factor. Used to thicken stroked outlines
    /// proportionally so a 2× scaled placement renders 2×-thick strokes.
    pub fn uniform_scale(&self) -> f32 {
        (self.a11 * self.a22 - self.a12 * self.a21).abs().sqrt()
    }
}
