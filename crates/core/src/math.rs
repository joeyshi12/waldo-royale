//! Minimal dependency-free 3D math for worldgen.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

impl Vec3 {
    pub const ZERO: Vec3 = v3(0.0, 0.0, 0.0);
    pub const Y: Vec3 = v3(0.0, 1.0, 0.0);

    pub fn add(self, o: Vec3) -> Vec3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: Vec3) -> Vec3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn scale(self, s: f32) -> Vec3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        v3(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn distance(self, o: Vec3) -> f32 {
        self.sub(o).length()
    }
    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l > 1e-12 {
            self.scale(1.0 / l)
        } else {
            Vec3::Y
        }
    }
}

/// Quaternion (x, y, z, w), same convention as Three.js.
#[derive(Clone, Copy, Debug)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    /// Rotation taking unit vector `from` to `to` (Three.js `setFromUnitVectors`).
    pub fn from_unit_vectors(from: Vec3, to: Vec3) -> Quat {
        let r = from.dot(to) + 1.0;
        if r < 1e-8 {
            // opposite directions: pick any perpendicular axis
            if from.x.abs() > from.z.abs() {
                Quat { x: -from.y, y: from.x, z: 0.0, w: 0.0 }.normalize()
            } else {
                Quat { x: 0.0, y: -from.z, z: from.y, w: 0.0 }.normalize()
            }
        } else {
            let c = from.cross(to);
            Quat { x: c.x, y: c.y, z: c.z, w: r }.normalize()
        }
    }

    /// Rotation of `angle` radians about unit `axis`.
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Quat {
        let h = angle * 0.5;
        let s = h.sin();
        Quat { x: axis.x * s, y: axis.y * s, z: axis.z * s, w: h.cos() }
    }

    pub fn multiply(self, b: Quat) -> Quat {
        let a = self;
        Quat {
            x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
            w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        }
    }

    pub fn normalize(self) -> Quat {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if l < 1e-12 {
            Quat::IDENTITY
        } else {
            Quat { x: self.x / l, y: self.y / l, z: self.z / l, w: self.w / l }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_unit_vectors_rotates() {
        let q = Quat::from_unit_vectors(Vec3::Y, v3(1.0, 0.0, 0.0));
        let v = Vec3::Y;
        let u = v3(q.x, q.y, q.z);
        let rotated = u
            .scale(2.0 * u.dot(v))
            .add(v.scale(q.w * q.w - u.dot(u)))
            .add(u.cross(v).scale(2.0 * q.w));
        assert!(rotated.distance(v3(1.0, 0.0, 0.0)) < 1e-5);
    }

    #[test]
    fn opposite_vectors() {
        let q = Quat::from_unit_vectors(Vec3::Y, v3(0.0, -1.0, 0.0));
        let n = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
        assert!((n - 1.0).abs() < 1e-5);
    }
}
