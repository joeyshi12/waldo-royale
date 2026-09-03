//! World generation: everything a round needs, derived from a single seed.

use crate::math::{v3, Quat, Vec3};
use crate::rng::Rng;
use crate::shapes::{build_shape, MeshData, TAU};
use serde::Serialize;

/// One placed object: position + orientation (surface-aligned, random yaw) + scale.
#[derive(Serialize, Clone, Debug)]
pub struct Placement {
    pub pos: [f32; 3],
    pub quat: [f32; 4],
    pub scale: [f32; 3],
}

/// One supporting-cast character on the surface.
#[derive(Serialize, Clone, Debug)]
pub struct CastPlacement {
    pub role: String,
    pub pos: [f32; 3],
    pub quat: [f32; 4],
    pub scale: [f32; 3],
}

/// Knobs a round mutator can turn. Both client and server derive these from
/// the mutator name, keeping generation deterministic.
#[derive(Clone, Copy)]
pub struct GenOptions {
    pub density_mult: f32,
    pub size_mult: f32,
}

impl Default for GenOptions {
    fn default() -> Self {
        GenOptions { density_mult: 1.0, size_mult: 1.0 }
    }
}

pub fn mutator_opts(mutator: &str) -> GenOptions {
    match mutator {
        "crowded" => GenOptions { density_mult: 1.7, size_mult: 1.0 },
        "tiny" => GenOptions { density_mult: 1.0, size_mult: 0.62 },
        _ => GenOptions::default(),
    }
}

/// A batch of same-kind scenery for instanced rendering.
/// `transforms` is 10 floats per instance: pos(3) quat(4) scale(3).
#[derive(Serialize)]
pub struct InstanceSet {
    pub kind: &'static str,
    pub count: u32,
    #[serde(skip)]
    pub transforms: Vec<f32>,
    #[serde(skip)]
    pub colors: Vec<f32>,
    #[serde(skip)]
    pub colors2: Vec<f32>,
}

#[derive(Serialize)]
pub struct World {
    pub seed: u32,
    pub shape_name: String,
    pub bounding_radius: f32,
    pub waldo: Placement,
    pub cast: Vec<CastPlacement>,
    pub sets: Vec<InstanceSet>,
    #[serde(skip)]
    pub mesh: MeshData,
}

impl World {
    pub fn waldo_pos(&self) -> Vec3 {
        v3(self.waldo.pos[0], self.waldo.pos[1], self.waldo.pos[2])
    }
}

/// Area-weighted random sampling of a triangle mesh surface.
pub struct SurfaceSampler<'a> {
    mesh: &'a MeshData,
    cumulative: Vec<f32>,
    total_area: f32,
}

impl<'a> SurfaceSampler<'a> {
    pub fn new(mesh: &'a MeshData) -> Self {
        let mut cumulative = Vec::with_capacity(mesh.indices.len() / 3);
        let mut total = 0.0f32;
        for tri in mesh.indices.chunks_exact(3) {
            let (a, b, c) = (
                mesh.position(tri[0] as usize),
                mesh.position(tri[1] as usize),
                mesh.position(tri[2] as usize),
            );
            total += b.sub(a).cross(c.sub(a)).length() * 0.5;
            cumulative.push(total);
        }
        SurfaceSampler { mesh, cumulative, total_area: total }
    }

    pub fn total_area(&self) -> f32 {
        self.total_area
    }

    pub fn sample(&self, rng: &mut Rng) -> (Vec3, Vec3) {
        let target = rng.f32() * self.total_area;
        let ti = self.cumulative.partition_point(|&a| a < target).min(self.cumulative.len() - 1);
        let tri = &self.mesh.indices[ti * 3..ti * 3 + 3];

        let r1 = rng.f32().sqrt();
        let r2 = rng.f32();
        let (wa, wb, wc) = (1.0 - r1, r1 * (1.0 - r2), r1 * r2);

        let vert = |k: usize| self.mesh.position(tri[k] as usize);
        let norm = |k: usize| {
            let i = tri[k] as usize;
            v3(
                self.mesh.normals[i * 3],
                self.mesh.normals[i * 3 + 1],
                self.mesh.normals[i * 3 + 2],
            )
        };
        let pos = vert(0).scale(wa).add(vert(1).scale(wb)).add(vert(2).scale(wc));
        let n = norm(0).scale(wa).add(norm(1).scale(wb)).add(norm(2).scale(wc)).normalize();
        (pos, n)
    }
}

fn hex(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 255) as f32 / 255.0,
        ((c >> 8) & 255) as f32 / 255.0,
        (c & 255) as f32 / 255.0,
    ]
}

fn surface_quat(rng: &mut Rng, normal: Vec3) -> Quat {
    Quat::from_unit_vectors(Vec3::Y, normal)
        .multiply(Quat::from_axis_angle(Vec3::Y, rng.range(0.0, TAU)))
}

const GREENS: [u32; 5] = [0x2e7d32, 0x388e3c, 0x1b5e20, 0x43a047, 0x33691e];
const BUILDINGS: [u32; 8] =
    [0xb9c0c9, 0x8f9aa8, 0xd8cfc0, 0xc96f5a, 0x7a8ea3, 0xe0d6b8, 0x9c8d7d, 0x6d7b8d];
const ROOFS: [u32; 3] = [0xc0392b, 0xa93226, 0xd35400];
const WALLS: [u32; 3] = [0xf5e6c8, 0xe8d5b0, 0xdfd0c0];
const TOWERS: [u32; 5] = [0xc9b8a0, 0xa8b8c8, 0xd0c0b0, 0x9098a8, 0xb0a890];
const DOMES: [u32; 4] = [0xd9b84a, 0xe8e8e0, 0x5aa0a8, 0xc07858];
const CARS: [u32; 6] = [0xd94040, 0x4070d9, 0xe8c030, 0x40b060, 0xe8e8e8, 0x303038];

struct KindSpec {
    kind: &'static str,
    count_min: f32,
    count_max: f32,
    clearance: f32, // min distance to Waldo
    palette: &'static [u32],
    palette2: &'static [u32],
}

const KINDS: [KindSpec; 11] = [
    KindSpec { kind: "pine", count_min: 220.0, count_max: 420.0, clearance: 0.8, palette: &GREENS, palette2: &[] },
    KindSpec { kind: "tree", count_min: 90.0, count_max: 200.0, clearance: 0.8, palette: &GREENS, palette2: &[] },
    KindSpec { kind: "bush", count_min: 120.0, count_max: 260.0, clearance: 0.6, palette: &GREENS, palette2: &[] },
    KindSpec { kind: "building", count_min: 120.0, count_max: 240.0, clearance: 1.6, palette: &BUILDINGS, palette2: &[] },
    KindSpec { kind: "house", count_min: 45.0, count_max: 100.0, clearance: 1.4, palette: &ROOFS, palette2: &WALLS },
    KindSpec { kind: "tower", count_min: 25.0, count_max: 60.0, clearance: 1.6, palette: &TOWERS, palette2: &[] },
    KindSpec { kind: "dome", count_min: 15.0, count_max: 40.0, clearance: 1.4, palette: &DOMES, palette2: &[] },
    KindSpec { kind: "pond", count_min: 12.0, count_max: 30.0, clearance: 1.2, palette: &[], palette2: &[] },
    KindSpec { kind: "car", count_min: 30.0, count_max: 80.0, clearance: 0.6, palette: &CARS, palette2: &[] },
    KindSpec { kind: "rock", count_min: 50.0, count_max: 120.0, clearance: 0.7, palette: &[], palette2: &[] },
    KindSpec { kind: "decoy", count_min: 5.0, count_max: 12.0, clearance: 3.5, palette: &[], palette2: &[] },
];

fn kind_scale(kind: &str, rng: &mut Rng) -> [f32; 3] {
    match kind {
        "pine" => {
            let s = rng.range(0.7, 1.5);
            [s, s * rng.range(0.85, 1.3), s]
        }
        "tree" => {
            let s = rng.range(0.7, 1.4);
            [s * rng.range(0.9, 1.2), s, s * rng.range(0.9, 1.2)]
        }
        "bush" => {
            let s = rng.range(0.6, 1.5);
            [s * rng.range(1.0, 1.4), s * rng.range(0.6, 0.9), s]
        }
        "building" => [rng.range(0.35, 0.9), rng.range(0.5, 2.3), rng.range(0.35, 0.9)],
        "house" => {
            let s = rng.range(0.8, 1.3);
            [s, s, s]
        }
        "tower" => [rng.range(0.7, 1.1), rng.range(0.9, 2.6), rng.range(0.7, 1.1)],
        "dome" => {
            let s = rng.range(0.7, 1.6);
            [s, s * rng.range(0.8, 1.1), s]
        }
        "pond" => [rng.range(0.7, 2.2), 1.0, rng.range(0.7, 2.2)],
        "car" => {
            let s = rng.range(0.8, 1.2);
            [s, s, s]
        }
        "rock" => {
            let s = rng.range(0.5, 1.6);
            [s, s * rng.range(0.6, 1.0), s]
        }
        _ => [1.0, rng.range(0.8, 1.2), 1.0], // decoy
    }
}

/// Scenery density is normalized against an average donut's surface area.
const BASELINE_AREA: f32 = 1450.0;

pub fn generate_world(seed: u32) -> World {
    generate_world_opts(seed, GenOptions::default())
}

pub fn generate_world_opts(seed: u32, opts: GenOptions) -> World {
    let mut rng = Rng::new(seed as u64);
    let mut shape = build_shape(&mut rng);
    if (opts.size_mult - 1.0).abs() > 1e-6 {
        for p in shape.mesh.positions.iter_mut() {
            *p *= opts.size_mult;
        }
        shape.bounding_radius *= opts.size_mult;
    }
    let sampler = SurfaceSampler::new(&shape.mesh);
    let density =
        ((sampler.total_area() / BASELINE_AREA).clamp(0.35, 1.6) * opts.density_mult).min(2.6);

    let (wpos, wnorm) = sampler.sample(&mut rng);
    let waldo = Placement {
        pos: [wpos.x, wpos.y, wpos.z],
        quat: {
            let q = surface_quat(&mut rng, wnorm);
            [q.x, q.y, q.z, q.w]
        },
        scale: [1.5, 1.5, 1.5],
    };

    // supporting cast: spaced away from Waldo and each other
    let roles: [(&str, f32); 4] =
        [("wenda", 1.5), ("odlaw", 1.5), ("woof", 0.9), ("wizard", 1.6)];
    let min_from_waldo = 3.0 * opts.size_mult;
    let min_between = 2.5 * opts.size_mult;
    let mut cast: Vec<CastPlacement> = Vec::with_capacity(roles.len());
    for (role, scale) in roles {
        let mut guard = 0;
        loop {
            guard += 1;
            let (pos, normal) = sampler.sample(&mut rng);
            let ok = pos.distance(wpos) >= min_from_waldo
                && cast.iter().all(|c| {
                    pos.distance(v3(c.pos[0], c.pos[1], c.pos[2])) >= min_between
                });
            if ok || guard > 100 {
                let q = surface_quat(&mut rng, normal);
                cast.push(CastPlacement {
                    role: role.to_string(),
                    pos: [pos.x, pos.y, pos.z],
                    quat: [q.x, q.y, q.z, q.w],
                    scale: [scale, scale, scale],
                });
                break;
            }
        }
    }

    let mut sets = Vec::with_capacity(KINDS.len());
    for spec in KINDS.iter() {
        let count =
            (rng.range(spec.count_min, spec.count_max) * density).round().max(1.0) as usize;
        let mut transforms = Vec::with_capacity(count * 10);
        let mut colors = Vec::with_capacity(count * 3);
        let mut colors2 = Vec::new();
        let mut placed = 0u32;
        let mut guard = 0;
        while (placed as usize) < count && guard < count * 30 {
            guard += 1;
            let (pos, normal) = sampler.sample(&mut rng);
            if pos.distance(wpos) < spec.clearance {
                continue;
            }
            if cast.iter().any(|c| pos.distance(v3(c.pos[0], c.pos[1], c.pos[2])) < 0.6) {
                continue;
            }
            let q = surface_quat(&mut rng, normal);
            let s = kind_scale(spec.kind, &mut rng);
            transforms.extend_from_slice(&[
                pos.x, pos.y, pos.z, q.x, q.y, q.z, q.w, s[0], s[1], s[2],
            ]);
            if !spec.palette.is_empty() {
                colors.extend_from_slice(&hex(spec.palette[rng.below(spec.palette.len())]));
            }
            if !spec.palette2.is_empty() {
                colors2.extend_from_slice(&hex(spec.palette2[rng.below(spec.palette2.len())]));
            }
            placed += 1;
        }
        sets.push(InstanceSet { kind: spec.kind, count: placed, transforms, colors, colors2 });
    }

    World {
        seed,
        shape_name: shape.name,
        bounding_radius: shape.bounding_radius,
        waldo,
        cast,
        sets,
        mesh: shape.mesh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_across_runs() {
        let a = generate_world(1234);
        let b = generate_world(1234);
        assert_eq!(a.shape_name, b.shape_name);
        assert_eq!(a.waldo.pos, b.waldo.pos);
        assert_eq!(a.mesh.positions, b.mesh.positions);
        assert_eq!(a.mesh.indices, b.mesh.indices);
        assert_eq!(a.sets.len(), b.sets.len());
        for (x, y) in a.sets.iter().zip(b.sets.iter()) {
            assert_eq!(x.transforms, y.transforms);
            assert_eq!(x.colors, y.colors);
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = generate_world(1);
        let b = generate_world(2);
        assert!(a.waldo.pos != b.waldo.pos || a.shape_name != b.shape_name);
    }

    #[test]
    fn waldo_is_on_surface() {
        for seed in 0..20 {
            let w = generate_world(seed);
            let d = w.waldo_pos().length();
            assert!(d <= w.bounding_radius + 1e-3, "waldo outside bounds");
            assert!(d > 1.0, "waldo at origin?");
        }
    }

    #[test]
    fn cast_is_placed_and_spaced() {
        for seed in 0..10 {
            let w = generate_world(seed);
            assert_eq!(w.cast.len(), 4);
            let roles: Vec<&str> = w.cast.iter().map(|c| c.role.as_str()).collect();
            assert_eq!(roles, ["wenda", "odlaw", "woof", "wizard"]);
            for c in &w.cast {
                let p = v3(c.pos[0], c.pos[1], c.pos[2]);
                assert!(p.length() <= w.bounding_radius + 1e-3);
            }
        }
    }

    #[test]
    fn mutators_change_generation() {
        let base = generate_world(42);
        let tiny = generate_world_opts(42, mutator_opts("tiny"));
        assert!(tiny.bounding_radius < base.bounding_radius * 0.7);
        let crowded = generate_world_opts(42, mutator_opts("crowded"));
        let count = |w: &World| w.sets.iter().map(|s| s.count).sum::<u32>();
        assert!(count(&crowded) > count(&base));
    }

    #[test]
    fn scenery_respects_clearance() {
        let w = generate_world(777);
        let wp = w.waldo_pos();
        for set in &w.sets {
            let clearance = KINDS.iter().find(|k| k.kind == set.kind).unwrap().clearance;
            for t in set.transforms.chunks_exact(10) {
                let d = v3(t[0], t[1], t[2]).distance(wp);
                assert!(d >= clearance - 1e-4, "{} too close: {d} < {clearance}", set.kind);
            }
        }
    }

    #[test]
    fn sampler_points_lie_within_bounds() {
        let mut rng = Rng::new(5);
        let shape = build_shape(&mut rng);
        let sampler = SurfaceSampler::new(&shape.mesh);
        for _ in 0..500 {
            let (p, n) = sampler.sample(&mut rng);
            assert!(p.length() <= shape.bounding_radius + 1e-3);
            assert!((n.length() - 1.0).abs() < 1e-3);
        }
    }
}
