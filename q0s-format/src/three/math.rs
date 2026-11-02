//! Small 3D math: vectors, quaternions and column-major 4×4 matrices.
//!
//! Stage space is right-handed with x to the right, y down and z away from
//! the viewer, so a 2D drawing lies in the z = 0 plane exactly as drawn.
//! Models are authored y-up (the usual 3D convention) and turned into stage
//! space when placed.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0);
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub const fn splat(v: f32) -> Self {
        Self::new(v, v, v)
    }

    pub fn from_array(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }

    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn length_sq(self) -> f32 {
        self.dot(self)
    }

    /// Unit length, or zero for a zero vector.
    pub fn normalized(self) -> Self {
        let len = self.length();
        if len > 1.0e-12 {
            self / len
        } else {
            Self::ZERO
        }
    }

    pub fn lerp(self, o: Self, t: f32) -> Self {
        self + (o - self) * t
    }

    pub fn min(self, o: Self) -> Self {
        Self::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }

    pub fn max(self, o: Self) -> Self {
        Self::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }

    pub fn mul_elem(self, o: Self) -> Self {
        Self::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }

    pub fn max_elem(self) -> f32 {
        self.x.max(self.y).max(self.z)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    pub fn distance(self, o: Self) -> f32 {
        (self - o).length()
    }

    /// Some unit vector at right angles to this one.
    pub fn any_perpendicular(self) -> Self {
        let other = if self.x.abs() < 0.9 { Self::X } else { Self::Y };
        self.cross(other).normalized()
    }
}

impl Add for V3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl AddAssign for V3 {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl Sub for V3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl SubAssign for V3 {
    fn sub_assign(&mut self, o: Self) {
        *self = *self - o;
    }
}

impl Mul<f32> for V3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Div<f32> for V3 {
    type Output = Self;
    fn div(self, s: f32) -> Self {
        Self::new(self.x / s, self.y / s, self.z / s)
    }
}

impl Neg for V3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// A rotation. Hamilton convention, `w` last.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    pub fn from_axis_angle(axis: V3, radians: f32) -> Self {
        let axis = axis.normalized();
        let (s, c) = (radians * 0.5).sin_cos();
        Self::new(axis.x * s, axis.y * s, axis.z * s, c)
    }

    /// Rotation by `x`, then `y`, then `z` degrees about the fixed axes
    /// (the usual "XYZ" Euler order of FBX and most editors).
    pub fn from_euler_deg(x: f32, y: f32, z: f32) -> Self {
        let qx = Self::from_axis_angle(V3::X, x.to_radians());
        let qy = Self::from_axis_angle(V3::Y, y.to_radians());
        let qz = Self::from_axis_angle(V3::Z, z.to_radians());
        qz * qy * qx
    }

    /// Degrees about x, y and z, the inverse of [`Quat::from_euler_deg`].
    pub fn to_euler_deg(self) -> V3 {
        let m = Mat4::from_quat(self);
        // m = Rz * Ry * Rx; column-major element (row r, col c) at c*4+r.
        let r20 = m.at(2, 0);
        let y = (-r20).clamp(-1.0, 1.0).asin();
        let (x, z) = if r20.abs() < 0.9999 {
            (m.at(2, 1).atan2(m.at(2, 2)), m.at(1, 0).atan2(m.at(0, 0)))
        } else {
            ((-m.at(1, 2)).atan2(m.at(1, 1)), 0.0)
        };
        V3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }

    pub fn normalized(self) -> Self {
        let len = self.dot(self).sqrt();
        if len > 1.0e-12 && len.is_finite() {
            Self::new(self.x / len, self.y / len, self.z / len, self.w / len)
        } else {
            Self::IDENTITY
        }
    }

    pub fn conjugate(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, self.w)
    }

    pub fn rotate(self, v: V3) -> V3 {
        let q = V3::new(self.x, self.y, self.z);
        let t = q.cross(v) * 2.0;
        v + t * self.w + q.cross(t)
    }

    /// Shortest-path spherical interpolation.
    pub fn slerp(self, o: Self, t: f32) -> Self {
        let mut o = o;
        let mut d = self.dot(o);
        if d < 0.0 {
            o = Self::new(-o.x, -o.y, -o.z, -o.w);
            d = -d;
        }
        if d > 0.9995 {
            return Self::new(
                self.x + (o.x - self.x) * t,
                self.y + (o.y - self.y) * t,
                self.z + (o.z - self.z) * t,
                self.w + (o.w - self.w) * t,
            )
            .normalized();
        }
        let theta = d.clamp(-1.0, 1.0).acos();
        let s = theta.sin();
        let a = ((1.0 - t) * theta).sin() / s;
        let b = (t * theta).sin() / s;
        Self::new(
            self.x * a + o.x * b,
            self.y * a + o.y * b,
            self.z * a + o.z * b,
            self.w * a + o.w * b,
        )
    }

    /// The rotation taking unit vector `from` onto unit vector `to`.
    pub fn between(from: V3, to: V3) -> Self {
        let from = from.normalized();
        let to = to.normalized();
        let d = from.dot(to);
        if d > 0.999_999 {
            return Self::IDENTITY;
        }
        if d < -0.999_999 {
            return Self::from_axis_angle(from.any_perpendicular(), std::f32::consts::PI);
        }
        let axis = from.cross(to);
        Self::new(axis.x, axis.y, axis.z, 1.0 + d).normalized()
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }
}

impl Mul for Quat {
    type Output = Self;
    /// `a * b` rotates by `b` first, then `a`.
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        )
    }
}

/// Column-major 4×4 matrix: element (row r, column c) is `m[c * 4 + r]`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Mat4 {
    pub m: [f32; 16],
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        m: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    };

    pub fn at(&self, row: usize, col: usize) -> f32 {
        self.m[col * 4 + row]
    }

    pub fn set(&mut self, row: usize, col: usize, value: f32) {
        self.m[col * 4 + row] = value;
    }

    pub fn from_rows(rows: [[f32; 4]; 4]) -> Self {
        let mut out = Self::IDENTITY;
        for (r, row) in rows.iter().enumerate() {
            for (c, value) in row.iter().enumerate() {
                out.set(r, c, *value);
            }
        }
        out
    }

    pub fn translation(t: V3) -> Self {
        let mut out = Self::IDENTITY;
        out.m[12] = t.x;
        out.m[13] = t.y;
        out.m[14] = t.z;
        out
    }

    pub fn scale(s: V3) -> Self {
        let mut out = Self::IDENTITY;
        out.m[0] = s.x;
        out.m[5] = s.y;
        out.m[10] = s.z;
        out
    }

    pub fn from_quat(q: Quat) -> Self {
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, yy, zz) = (x * x2, y * y2, z * z2);
        let (xy, xz, yz) = (x * y2, x * z2, y * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);
        Self {
            m: [
                1.0 - (yy + zz),
                xy + wz,
                xz - wy,
                0.0,
                xy - wz,
                1.0 - (xx + zz),
                yz + wx,
                0.0,
                xz + wy,
                yz - wx,
                1.0 - (xx + yy),
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ],
        }
    }

    /// Scale, then rotate, then translate.
    pub fn from_trs(t: V3, r: Quat, s: V3) -> Self {
        let mut out = Self::from_quat(r);
        for i in 0..3 {
            out.m[i] *= s.x;
            out.m[4 + i] *= s.y;
            out.m[8 + i] *= s.z;
        }
        out.m[12] = t.x;
        out.m[13] = t.y;
        out.m[14] = t.z;
        out
    }

    pub fn transform_point(&self, p: V3) -> V3 {
        let m = &self.m;
        V3::new(
            m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12],
            m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13],
            m[2] * p.x + m[6] * p.y + m[10] * p.z + m[14],
        )
    }

    pub fn transform_vector(&self, v: V3) -> V3 {
        let m = &self.m;
        V3::new(
            m[0] * v.x + m[4] * v.y + m[8] * v.z,
            m[1] * v.x + m[5] * v.y + m[9] * v.z,
            m[2] * v.x + m[6] * v.y + m[10] * v.z,
        )
    }

    /// The full product with w = 1, before any divide.
    pub fn transform4(&self, p: V3) -> [f32; 4] {
        let m = &self.m;
        [
            m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12],
            m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13],
            m[2] * p.x + m[6] * p.y + m[10] * p.z + m[14],
            m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15],
        ]
    }

    pub fn translation_part(&self) -> V3 {
        V3::new(self.m[12], self.m[13], self.m[14])
    }

    pub fn column(&self, c: usize) -> V3 {
        V3::new(self.m[c * 4], self.m[c * 4 + 1], self.m[c * 4 + 2])
    }

    pub fn determinant3(&self) -> f32 {
        let m = &self.m;
        m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2])
            + m[8] * (m[1] * m[6] - m[5] * m[2])
    }

    pub fn transpose(&self) -> Self {
        let mut out = Self::IDENTITY;
        for r in 0..4 {
            for c in 0..4 {
                out.set(r, c, self.at(c, r));
            }
        }
        out
    }

    pub fn inverse(&self) -> Option<Self> {
        let m = &self.m;
        let mut inv = [0.0f32; 16];
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
            + m[9] * m[7] * m[14]
            + m[13] * m[6] * m[11]
            - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
            - m[8] * m[7] * m[14]
            - m[12] * m[6] * m[11]
            + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
            + m[8] * m[7] * m[13]
            + m[12] * m[5] * m[11]
            - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
            - m[8] * m[6] * m[13]
            - m[12] * m[5] * m[10]
            + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
            - m[9] * m[3] * m[14]
            - m[13] * m[2] * m[11]
            + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
            + m[8] * m[3] * m[14]
            + m[12] * m[2] * m[11]
            - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
            - m[8] * m[3] * m[13]
            - m[12] * m[1] * m[11]
            + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
            + m[8] * m[2] * m[13]
            + m[12] * m[1] * m[10]
            - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
            + m[5] * m[3] * m[14]
            + m[13] * m[2] * m[7]
            - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
            - m[4] * m[3] * m[14]
            - m[12] * m[2] * m[7]
            + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
            + m[4] * m[3] * m[13]
            + m[12] * m[1] * m[7]
            - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
            - m[4] * m[2] * m[13]
            - m[12] * m[1] * m[6]
            + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
            - m[5] * m[3] * m[10]
            - m[9] * m[2] * m[7]
            + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
            + m[4] * m[3] * m[10]
            + m[8] * m[2] * m[7]
            - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
            - m[4] * m[3] * m[9]
            - m[8] * m[1] * m[7]
            + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
            + m[4] * m[2] * m[9]
            + m[8] * m[1] * m[6]
            - m[8] * m[2] * m[5];
        let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        if det.abs() < 1.0e-20 || !det.is_finite() {
            return None;
        }
        let inv_det = 1.0 / det;
        for value in &mut inv {
            *value *= inv_det;
        }
        Some(Self { m: inv })
    }

    /// Translation, rotation and scale of an affine matrix without shear.
    /// A mirrored matrix comes back with a negative x scale.
    pub fn decompose(&self) -> (V3, Quat, V3) {
        let t = self.translation_part();
        let mut sx = self.column(0).length();
        let sy = self.column(1).length();
        let sz = self.column(2).length();
        if self.determinant3() < 0.0 {
            sx = -sx;
        }
        let safe = |s: f32| if s.abs() > 1.0e-12 { s } else { 1.0 };
        let c0 = self.column(0) / safe(sx);
        let c1 = self.column(1) / safe(sy);
        let c2 = self.column(2) / safe(sz);
        let rot = quat_from_basis(c0, c1, c2);
        (t, rot, V3::new(sx, sy, sz))
    }

    pub fn is_finite(&self) -> bool {
        self.m.iter().all(|value| value.is_finite())
    }
}

impl Mul for Mat4 {
    type Output = Self;
    /// `a * b` applies `b` first, then `a`.
    fn mul(self, o: Self) -> Self {
        let mut out = [0.0f32; 16];
        for c in 0..4 {
            for r in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += self.m[k * 4 + r] * o.m[c * 4 + k];
                }
                out[c * 4 + r] = sum;
            }
        }
        Self { m: out }
    }
}

/// The rotation whose matrix has columns `x`, `y` and `z`.
pub fn quat_from_basis(x: V3, y: V3, z: V3) -> Quat {
    let (m00, m11, m22) = (x.x, y.y, z.z);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat::new((y.z - z.y) / s, (z.x - x.z) / s, (x.y - y.x) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        Quat::new(0.25 * s, (y.x + x.y) / s, (z.x + x.z) / s, (y.z - z.y) / s)
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        Quat::new((y.x + x.y) / s, 0.25 * s, (z.y + y.z) / s, (z.x - x.z) / s)
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        Quat::new((z.x + x.z) / s, (z.y + y.z) / s, 0.25 * s, (x.y - y.x) / s)
    };
    q.normalized()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (a - b).length() < 1.0e-3
    }

    #[test]
    fn quaternions_and_matrices_agree() {
        let q = Quat::from_euler_deg(30.0, -45.0, 70.0);
        let m = Mat4::from_quat(q);
        let v = V3::new(1.0, 2.0, 3.0);
        assert!(close(q.rotate(v), m.transform_vector(v)));
        let e = q.to_euler_deg();
        assert!(
            (e.x - 30.0).abs() < 0.01 && (e.y + 45.0).abs() < 0.01 && (e.z - 70.0).abs() < 0.01
        );
    }

    #[test]
    fn a_matrix_comes_apart_and_back_together() {
        let m = Mat4::from_trs(
            V3::new(4.0, -2.0, 9.0),
            Quat::from_euler_deg(10.0, 20.0, 30.0),
            V3::new(2.0, 3.0, 0.5),
        );
        let inverse = m.inverse().unwrap();
        let p = V3::new(-3.0, 5.0, 1.0);
        assert!(close(inverse.transform_point(m.transform_point(p)), p));
        let (t, r, s) = m.decompose();
        let again = Mat4::from_trs(t, r, s);
        assert!(close(again.transform_point(p), m.transform_point(p)));
    }

    #[test]
    fn between_turns_one_way_onto_another() {
        let q = Quat::between(V3::X, V3::new(0.0, 1.0, 1.0));
        assert!(close(q.rotate(V3::X), V3::new(0.0, 1.0, 1.0).normalized()));
    }
}
