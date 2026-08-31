//! Procedural planet meshes. Every shape is built from parametric patches
//! with analytic normals, so the exact same vertex data is produced on
//! native (server) and wasm32 (client) from the same RNG stream.

use crate::math::{v3, Vec3};
use crate::rng::Rng;
use std::collections::HashMap;

pub const TAU: f32 = std::f32::consts::TAU;

#[derive(Default)]
pub struct MeshData {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub uvs: Vec<f32>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    pub fn position(&self, i: usize) -> Vec3 {
        v3(
            self.positions[i * 3],
            self.positions[i * 3 + 1],
            self.positions[i * 3 + 2],
        )
    }
}

pub struct Shape {
    pub name: String,
    pub mesh: MeshData,
    pub bounding_radius: f32,
}

/// Tessellate one parametric patch over (u, v) ∈ [0,1]² into `mesh`.
fn add_patch<P, N>(mesh: &mut MeshData, nu: usize, nv: usize, pos_fn: P, normal_fn: N)
where
    P: Fn(f32, f32) -> Vec3,
    N: Fn(f32, f32) -> Vec3,
{
    let base = mesh.vertex_count() as u32;
    for i in 0..=nu {
        let u = i as f32 / nu as f32;
        for j in 0..=nv {
            let v = j as f32 / nv as f32;
            let p = pos_fn(u, v);
            let n = normal_fn(u, v).normalize();
            mesh.positions.extend_from_slice(&[p.x, p.y, p.z]);
            mesh.normals.extend_from_slice(&[n.x, n.y, n.z]);
            mesh.uvs.extend_from_slice(&[u, v]);
        }
    }
    let stride = (nv + 1) as u32;
    for i in 0..nu as u32 {
        for j in 0..nv as u32 {
            let a = base + i * stride + j;
            let b = a + stride;
            mesh.indices.extend_from_slice(&[a, b, a + 1, b, b + 1, a + 1]);
        }
    }
}

/// Wavy radial displacement. Recomputes normals from faces afterwards,
/// welding coincident seam vertices and preserving outward orientation.
pub fn warp(mesh: &mut MeshData, rng: &mut Rng, amp: f32) {
    let f1 = rng.range(0.2, 0.55);
    let f2 = rng.range(0.2, 0.55);
    let f3 = rng.range(0.2, 0.55);
    let old_normals = mesh.normals.clone();

    for i in 0..mesh.vertex_count() {
        let p = mesh.position(i);
        let bump =
            1.0 + amp * ((p.x * f1).sin() + (p.y * f2).sin() + (p.z * f3).sin()) / 3.0;
        let q = p.scale(bump);
        mesh.positions[i * 3] = q.x;
        mesh.positions[i * 3 + 1] = q.y;
        mesh.positions[i * 3 + 2] = q.z;
    }

    // accumulate face normals per welded position
    let key = |p: Vec3| -> (i32, i32, i32) {
        (
            (p.x * 64.0).round() as i32,
            (p.y * 64.0).round() as i32,
            (p.z * 64.0).round() as i32,
        )
    };
    let mut acc: HashMap<(i32, i32, i32), Vec3> = HashMap::new();
    for tri in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (
            mesh.position(tri[0] as usize),
            mesh.position(tri[1] as usize),
            mesh.position(tri[2] as usize),
        );
        let fnorm = b.sub(a).cross(c.sub(a)); // area-weighted
        for &v in tri {
            let e = acc.entry(key(mesh.position(v as usize))).or_insert(Vec3::ZERO);
            *e = e.add(fnorm);
        }
    }
    for i in 0..mesh.vertex_count() {
        let mut n = acc[&key(mesh.position(i))].normalize();
        // keep pointing the same way as before the warp (outward)
        let old = v3(old_normals[i * 3], old_normals[i * 3 + 1], old_normals[i * 3 + 2]);
        if n.dot(old) < 0.0 {
            n = n.scale(-1.0);
        }
        mesh.normals[i * 3] = n.x;
        mesh.normals[i * 3 + 1] = n.y;
        mesh.normals[i * 3 + 2] = n.z;
    }
}

fn bounding_radius(mesh: &MeshData) -> f32 {
    (0..mesh.vertex_count())
        .map(|i| mesh.position(i).length())
        .fold(0.0, f32::max)
}

const KNOT_WINDINGS: [(f32, f32); 6] =
    [(2.0, 3.0), (3.0, 2.0), (2.0, 5.0), (3.0, 4.0), (3.0, 5.0), (4.0, 3.0)];

fn torus(rng: &mut Rng) -> (MeshData, &'static str) {
    let big = rng.range(8.0, 11.0);
    let tube = rng.range(2.8, 5.0);
    let mut m = MeshData::default();
    add_patch(
        &mut m,
        160,
        72,
        |u, v| {
            let (t, p) = (u * TAU, v * TAU);
            v3(
                (big + tube * p.cos()) * t.cos(),
                tube * p.sin(),
                (big + tube * p.cos()) * t.sin(),
            )
        },
        |u, v| {
            let (t, p) = (u * TAU, v * TAU);
            v3(p.cos() * t.cos(), p.sin(), p.cos() * t.sin())
        },
    );
    (m, "Donut")
}

fn ellipsoid(rng: &mut Rng) -> (MeshData, &'static str) {
    let r = rng.range(9.5, 12.0);
    let (a, b, c) = (r, r * rng.range(0.55, 1.0), r * rng.range(0.7, 1.05));
    let mut m = MeshData::default();
    add_patch(
        &mut m,
        64,
        40,
        |u, v| {
            let (t, p) = (u * TAU, v * std::f32::consts::PI);
            v3(a * p.sin() * t.cos(), b * p.cos(), c * p.sin() * t.sin())
        },
        |u, v| {
            let (t, p) = (u * TAU, v * std::f32::consts::PI);
            // ellipsoid normal ∝ (x/a², y/b², z/c²)
            v3(p.sin() * t.cos() / a, p.cos() / b, p.sin() * t.sin() / c)
        },
    );
    (m, "Ellipsoid")
}

fn knot(rng: &mut Rng) -> (MeshData, &'static str) {
    let (p, q) = KNOT_WINDINGS[rng.below(KNOT_WINDINGS.len())];
    let radius = rng.range(6.0, 8.0);
    let tube = rng.range(1.7, 2.8);

    // Same curve as Three.js TorusKnotGeometry
    let curve = move |t: f32| -> Vec3 {
        let cs = (q * t / p).cos();
        v3(
            radius * (2.0 + cs) * 0.5 * t.cos(),
            radius * (2.0 + cs) * 0.5 * t.sin(),
            radius * (q * t / p).sin() * 0.5,
        )
    };
    let frame_point = move |u: f32, v: f32| -> (Vec3, Vec3) {
        let t = u * p * TAU;
        let p1 = curve(t);
        let p2 = curve(t + 0.01);
        let tangent = p2.sub(p1);
        let mut bnorm = tangent.cross(p2.add(p1));
        let nnorm = bnorm.cross(tangent).normalize();
        bnorm = bnorm.normalize();
        let ang = v * TAU;
        let offset = nnorm.scale(-tube * ang.cos()).add(bnorm.scale(tube * ang.sin()));
        (p1.add(offset), offset.normalize())
    };
    let mut m = MeshData::default();
    add_patch(
        &mut m,
        320,
        24,
        move |u, v| frame_point(u, v).0,
        move |u, v| frame_point(u, v).1,
    );
    (m, "Knot")
}

fn cube(rng: &mut Rng) -> (MeshData, &'static str) {
    let h = [
        rng.range(5.0, 8.5),
        rng.range(5.0, 8.5),
        rng.range(5.0, 8.5),
    ];
    let mut m = MeshData::default();
    // (axis, sign): axis is the face normal direction
    for (axis, sign) in [(0, 1.0f32), (0, -1.0), (1, 1.0), (1, -1.0), (2, 1.0), (2, -1.0)] {
        let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
        add_patch(
            &mut m,
            8,
            8,
            move |u, v| {
                let mut c = [0.0f32; 3];
                c[axis] = h[axis] * sign;
                c[a1] = h[a1] * (u * 2.0 - 1.0) * sign; // flip u with sign for outward winding
                c[a2] = h[a2] * (v * 2.0 - 1.0);
                v3(c[0], c[1], c[2])
            },
            move |_, _| {
                let mut n = [0.0f32; 3];
                n[axis] = sign;
                v3(n[0], n[1], n[2])
            },
        );
    }
    (m, "Cube")
}

fn capsule(rng: &mut Rng) -> (MeshData, &'static str) {
    let r = rng.range(4.5, 6.5);
    let hl = rng.range(3.0, 6.0);
    let mut m = MeshData::default();
    // side
    add_patch(
        &mut m,
        28,
        10,
        move |u, v| {
            let t = u * TAU;
            v3(r * t.cos(), -hl + 2.0 * hl * v, r * t.sin())
        },
        move |u, _| {
            let t = u * TAU;
            v3(t.cos(), 0.0, t.sin())
        },
    );
    // caps
    for sign in [1.0f32, -1.0] {
        add_patch(
            &mut m,
            28,
            10,
            move |u, v| {
                let t = u * TAU;
                let phi = v * std::f32::consts::FRAC_PI_2;
                v3(
                    r * phi.cos() * t.cos(),
                    sign * (hl + r * phi.sin()),
                    r * phi.cos() * t.sin(),
                )
            },
            move |u, v| {
                let t = u * TAU;
                let phi = v * std::f32::consts::FRAC_PI_2;
                v3(phi.cos() * t.cos(), sign * phi.sin(), phi.cos() * t.sin())
            },
        );
    }
    (m, "Capsule")
}

fn cone(rng: &mut Rng) -> (MeshData, &'static str) {
    let rt = rng.range(0.0, 6.0);
    let rb = rng.range(5.0, 8.5);
    let h = rng.range(9.0, 15.0);
    let hh = h * 0.5;
    let slope = (rt - rb) / h; // dr/dy
    let mut m = MeshData::default();
    // side
    add_patch(
        &mut m,
        28,
        10,
        move |u, v| {
            let t = u * TAU;
            let y = -hh + h * v;
            let r = rb + (rt - rb) * v;
            v3(r * t.cos(), y, r * t.sin())
        },
        move |u, _| {
            let t = u * TAU;
            v3(t.cos(), -slope, t.sin())
        },
    );
    // bottom cap
    add_patch(
        &mut m,
        28,
        4,
        move |u, v| {
            let t = u * TAU;
            let rho = rb * v;
            v3(rho * t.cos(), -hh, rho * t.sin())
        },
        |_, _| v3(0.0, -1.0, 0.0),
    );
    // top cap (skip if it degenerates to a point)
    if rt > 0.05 {
        add_patch(
            &mut m,
            28,
            4,
            move |u, v| {
                let t = u * TAU;
                let rho = rt * v;
                v3(rho * t.cos(), hh, rho * t.sin())
            },
            |_, _| v3(0.0, 1.0, 0.0),
        );
    }
    (m, "Cone")
}

fn asteroid(rng: &mut Rng) -> (MeshData, &'static str) {
    let r = rng.range(9.0, 11.0);
    let mut m = MeshData::default();
    add_patch(
        &mut m,
        64,
        40,
        move |u, v| {
            let (t, p) = (u * TAU, v * std::f32::consts::PI);
            v3(r * p.sin() * t.cos(), r * p.cos(), r * p.sin() * t.sin())
        },
        move |u, v| {
            let (t, p) = (u * TAU, v * std::f32::consts::PI);
            v3(p.sin() * t.cos(), p.cos(), p.sin() * t.sin())
        },
    );
    let amp = rng.range(0.15, 0.3);
    warp(&mut m, rng, amp);
    (m, "Asteroid")
}

/// Build a random planet shape from the RNG stream (deterministic per seed).
pub fn build_shape(rng: &mut Rng) -> Shape {
    let builders: [fn(&mut Rng) -> (MeshData, &'static str); 7] =
        [torus, ellipsoid, knot, cube, capsule, cone, asteroid];
    let idx = rng.below(builders.len());
    let (mut mesh, base_name) = builders[idx](rng);

    let mut name = base_name.to_string();
    if base_name != "Asteroid" && rng.f32() < 0.5 {
        let amp = rng.range(0.05, 0.16);
        warp(&mut mesh, rng, amp);
        name = format!("Wobbly {name}");
    }

    let bounding_radius = bounding_radius(&mesh);
    Shape { name, mesh, bounding_radius }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_mesh(m: &MeshData) {
        assert!(!m.indices.is_empty());
        assert_eq!(m.positions.len() % 3, 0);
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.uvs.len() / 2, m.vertex_count());
        let vc = m.vertex_count() as u32;
        assert!(m.indices.iter().all(|&i| i < vc), "index out of range");
        assert!(m.positions.iter().all(|p| p.is_finite()));
        for i in 0..m.vertex_count() {
            let n = v3(m.normals[i * 3], m.normals[i * 3 + 1], m.normals[i * 3 + 2]);
            assert!((n.length() - 1.0).abs() < 1e-3, "non-unit normal {n:?}");
        }
    }

    #[test]
    fn all_shapes_valid() {
        // enough seeds to hit every shape family + wobble branch
        for seed in 0..40u64 {
            let mut rng = Rng::new(seed);
            let s = build_shape(&mut rng);
            check_mesh(&s.mesh);
            assert!(s.bounding_radius > 4.0 && s.bounding_radius < 25.0, "{}", s.name);
        }
    }
}
