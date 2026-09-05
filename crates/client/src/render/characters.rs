//! Character and primitive-prop geometry, built from three-d primitives.
//! Stripes are geometry (alternating colored bands), not textures, so they
//! render identically everywhere.

use three_d::*;

pub const RED: Srgba = Srgba::new(224, 32, 32, 255);
pub const WHITE: Srgba = Srgba::new(255, 255, 255, 255);
pub const YELLOW: Srgba = Srgba::new(232, 192, 32, 255);
pub const BLACK: Srgba = Srgba::new(38, 38, 42, 255);
const SKIN: Srgba = Srgba::new(242, 200, 150, 255);
const JEANS: Srgba = Srgba::new(53, 88, 196, 255);
const DARK: Srgba = Srgba::new(42, 42, 46, 255);
const HAIR: Srgba = Srgba::new(74, 50, 32, 255);
const BROWN: Srgba = Srgba::new(138, 90, 43, 255);
const POND_BLUE: Srgba = Srgba::new(61, 120, 200, 255);

pub fn empty_mesh() -> CpuMesh {
    CpuMesh {
        positions: Positions::F32(Vec::new()),
        normals: Some(Vec::new()),
        indices: Indices::U32(Vec::new()),
        ..Default::default()
    }
}

/// Append `mesh` transformed by `m` into `out`.
pub fn append(out: &mut CpuMesh, mut mesh: CpuMesh, m: Mat4) {
    mesh.transform(m).unwrap();
    let Positions::F32(src) = &mesh.positions else { return };
    let Positions::F32(dst) = &mut out.positions else { return };
    let base = dst.len() as u32;
    dst.extend_from_slice(src);
    if let (Some(sn), Some(dn)) = (&mesh.normals, &mut out.normals) {
        dn.extend_from_slice(sn);
    }
    let src_idx: Vec<u32> = match &mesh.indices {
        Indices::U32(v) => v.clone(),
        Indices::U16(v) => v.iter().map(|&i| i as u32).collect(),
        Indices::U8(v) => v.iter().map(|&i| i as u32).collect(),
        Indices::None => (0..src.len() as u32).collect(),
    };
    if let Indices::U32(out_idx) = &mut out.indices {
        out_idx.extend(src_idx.iter().map(|i| i + base));
    }
}

/// A y-up cylinder/cone segment with base at origin: (primitive, transform).
pub fn cylinder_y(r_bottom: f32, r_top: f32, height: f32) -> (CpuMesh, Mat4) {
    let mesh = if (r_bottom - r_top).abs() < 1e-4 {
        CpuMesh::cylinder(14)
    } else {
        CpuMesh::cone(14)
    };
    let r = r_bottom.max(r_top);
    (mesh, Mat4::from_angle_z(degrees(90.0)) * Mat4::from_nonuniform_scale(height, r, r))
}

fn sphere_at(r: f32, x: f32, y: f32, z: f32) -> (CpuMesh, Mat4) {
    (CpuMesh::sphere(14), Mat4::from_translation(vec3(x, y, z)) * Mat4::from_scale(r))
}

fn box_at(sx: f32, sy: f32, sz: f32, x: f32, y: f32, z: f32) -> (CpuMesh, Mat4) {
    (
        CpuMesh::cube(),
        Mat4::from_translation(vec3(x, y, z))
            * Mat4::from_nonuniform_scale(sx / 2.0, sy / 2.0, sz / 2.0),
    )
}

/// Alternating-band cylinder: bands starting with `a` at the bottom.
/// Appends into two meshes so each color stays one draw call.
fn striped_cylinder(
    out_a: &mut CpuMesh,
    out_b: &mut CpuMesh,
    r_bottom: f32,
    r_top: f32,
    height: f32,
    bands: usize,
    base: Mat4,
) {
    let bh = height / bands as f32;
    for i in 0..bands {
        let t0 = i as f32 / bands as f32;
        let t1 = (i + 1) as f32 / bands as f32;
        let r0 = r_bottom + (r_top - r_bottom) * t0;
        let r1 = r_bottom + (r_top - r_bottom) * t1;
        let (mesh, m) = cylinder_y(r0, r1, bh);
        let placed = base * Mat4::from_translation(vec3(0.0, bh * i as f32, 0.0)) * m;
        let out = if i % 2 == 0 { &mut *out_a } else { &mut *out_b };
        append(out, mesh, placed);
    }
}

/// One character = a few (mesh, color) parts sharing the placement transform.
pub type Parts = Vec<(CpuMesh, Srgba)>;

fn shared_body(parts: &mut Parts, stripe_a: Srgba, stripe_b: Srgba, trousers: Option<Srgba>) {
    let mut a = empty_mesh();
    let mut b = empty_mesh();

    if let Some(color) = trousers {
        let mut legs = empty_mesh();
        let (c, m) = cylinder_y(0.03, 0.026, 0.17);
        append(&mut legs, c.clone(), Mat4::from_translation(vec3(0.042, 0.03, 0.0)) * m);
        append(&mut legs, c, Mat4::from_translation(vec3(-0.042, 0.03, 0.0)) * m);
        parts.push((legs, color));
        let mut shoes = empty_mesh();
        let (s, m) = box_at(0.055, 0.03, 0.09, 0.042, 0.015, 0.012);
        append(&mut shoes, s.clone(), m);
        let (_, m2) = box_at(0.055, 0.03, 0.09, -0.042, 0.015, 0.012);
        append(&mut shoes, s, m2);
        parts.push((shoes, DARK));
    }

    // torso + arms, striped
    striped_cylinder(&mut a, &mut b, 0.098, 0.082, 0.22, 5, Mat4::from_translation(vec3(0.0, 0.2, 0.0)));
    for side in [1.0f32, -1.0] {
        let arm = Mat4::from_translation(vec3(0.115 * side, 0.225, 0.0))
            * Mat4::from_angle_z(Rad(0.42 * side));
        striped_cylinder(&mut a, &mut b, 0.03, 0.026, 0.19, 4, arm);
    }
    parts.push((a, stripe_a));
    parts.push((b, stripe_b));

    // hands
    let mut hands = empty_mesh();
    let (h, m) = sphere_at(0.026, 0.156, 0.235, 0.0);
    append(&mut hands, h.clone(), m);
    let (_, m2) = sphere_at(0.026, -0.156, 0.235, 0.0);
    append(&mut hands, h, m2);
    parts.push((hands, SKIN));

    // head
    let mut head = empty_mesh();
    let (s, m) = sphere_at(0.085, 0.0, 0.5, 0.0);
    append(&mut head, s, m);
    parts.push((head, SKIN));

    // glasses: two flat lens discs + bridge
    let mut glasses = empty_mesh();
    let lens = Mat4::from_angle_x(degrees(90.0));
    for side in [1.0f32, -1.0] {
        let (c, m) = cylinder_y(0.026, 0.026, 0.008);
        append(
            &mut glasses,
            c,
            Mat4::from_translation(vec3(0.037 * side, 0.505, 0.078)) * lens * m,
        );
    }
    let (bridge, m) = box_at(0.024, 0.008, 0.008, 0.0, 0.508, 0.082);
    append(&mut glasses, bridge, m);
    parts.push((glasses, DARK));
}

fn hat(parts: &mut Parts, stripe_a: Srgba, stripe_b: Srgba, brim: Srgba, pom: Srgba) {
    let mut brim_mesh = empty_mesh();
    let (c, m) = cylinder_y(0.09, 0.088, 0.035);
    append(&mut brim_mesh, c, Mat4::from_translation(vec3(0.0, 0.555, 0.0)) * m);
    parts.push((brim_mesh, brim));

    let mut a = empty_mesh();
    let mut b = empty_mesh();
    striped_cylinder(&mut a, &mut b, 0.086, 0.065, 0.07, 3, Mat4::from_translation(vec3(0.0, 0.585, 0.0)));
    parts.push((a, stripe_a));
    parts.push((b, stripe_b));

    let mut p = empty_mesh();
    let (s, m) = sphere_at(0.028, 0.0, 0.68, 0.0);
    append(&mut p, s, m);
    parts.push((p, pom));
}

fn hair_cap(parts: &mut Parts, color: Srgba, long: bool) {
    let mut hair = empty_mesh();
    let (s, m) = sphere_at(0.088, 0.0, 0.515, -0.012);
    let squash = if long { vec3(1.0, 0.85, 1.0) } else { vec3(1.0, 0.62, 1.0) };
    append(&mut hair, s, m * Mat4::from_nonuniform_scale(squash.x, squash.y, squash.z));
    parts.push((hair, color));
}

pub fn build_character(role: &str) -> Parts {
    let mut parts: Parts = Vec::new();
    match role {
        "woof" => {
            let mut a = empty_mesh();
            let mut b = empty_mesh();
            striped_cylinder(&mut a, &mut b, 0.028, 0.02, 0.2, 5, Mat4::from_angle_z(Rad(0.15)));
            parts.push((a, RED));
            parts.push((b, WHITE));
            let mut tuft = empty_mesh();
            let (s, m) = sphere_at(0.04, 0.03, 0.215, 0.0);
            append(&mut tuft, s, m);
            parts.push((tuft, WHITE));
        }
        "wizard" => {
            let mut robe = empty_mesh();
            let (r, m) = cylinder_y(0.16, 0.05, 0.45);
            append(&mut robe, r, m);
            parts.push((robe, Srgba::new(192, 57, 43, 255)));
            let mut head = empty_mesh();
            let (s, m) = sphere_at(0.08, 0.0, 0.5, 0.0);
            append(&mut head, s, m);
            parts.push((head, SKIN));
            let mut beard = empty_mesh();
            let (bd, m) = cylinder_y(0.07, 0.015, 0.22);
            append(&mut beard, bd, Mat4::from_translation(vec3(0.0, 0.28, 0.05)) * Mat4::from_angle_x(Rad(0.25)) * m);
            parts.push((beard, Srgba::new(245, 245, 245, 255)));
            let mut hat_mesh = empty_mesh();
            let (h, m) = cylinder_y(0.09, 0.005, 0.2);
            append(&mut hat_mesh, h, Mat4::from_translation(vec3(0.0, 0.56, 0.0)) * m);
            parts.push((hat_mesh, Srgba::new(192, 57, 43, 255)));
            let mut staff = empty_mesh();
            let (st, m) = cylinder_y(0.012, 0.012, 0.55);
            append(&mut staff, st, Mat4::from_translation(vec3(0.16, 0.0, 0.0)) * Mat4::from_angle_z(Rad(-0.08)) * m);
            parts.push((staff, BROWN));
        }
        "wenda" => {
            let mut skirt = empty_mesh();
            let (c, m) = cylinder_y(0.12, 0.02, 0.16);
            append(&mut skirt, c, Mat4::from_translation(vec3(0.0, 0.1, 0.0)) * m);
            parts.push((skirt, JEANS));
            let mut a = empty_mesh();
            let mut b = empty_mesh();
            for side in [1.0f32, -1.0] {
                striped_cylinder(&mut a, &mut b, 0.026, 0.024, 0.14,
                    4, Mat4::from_translation(vec3(0.04 * side, 0.0, 0.0)));
            }
            parts.push((a, RED));
            parts.push((b, WHITE));
            shared_body(&mut parts, RED, WHITE, None);
            hair_cap(&mut parts, HAIR, true);
            hat(&mut parts, RED, WHITE, WHITE, RED);
        }
        "odlaw" => {
            shared_body(&mut parts, YELLOW, BLACK, Some(DARK));
            hair_cap(&mut parts, BLACK, false);
            let mut stache = empty_mesh();
            let (b, m) = box_at(0.07, 0.014, 0.02, 0.0, 0.462, 0.078);
            append(&mut stache, b, m);
            parts.push((stache, BLACK));
            hat(&mut parts, YELLOW, BLACK, BLACK, YELLOW);
        }
        _ => {
            // waldo
            shared_body(&mut parts, RED, WHITE, Some(JEANS));
            hair_cap(&mut parts, HAIR, false);
            hat(&mut parts, RED, WHITE, WHITE, RED);
            let mut cane = empty_mesh();
            let (c, m) = cylinder_y(0.009, 0.009, 0.3);
            append(&mut cane, c, Mat4::from_translation(vec3(0.185, 0.0, 0.02)) * Mat4::from_angle_z(Rad(-0.12)) * m);
            parts.push((cane, BROWN));
        }
    }
    parts
}

/// Procedural props still built from primitives: (mesh_a, color_a, mesh_b?, color_b).
pub fn pond() -> (CpuMesh, Srgba) {
    let mut m = empty_mesh();
    let (c, t) = cylinder_y(0.5, 0.5, 0.04);
    append(&mut m, c, t);
    (m, POND_BLUE)
}

pub fn decoy() -> (CpuMesh, CpuMesh) {
    let mut a = empty_mesh();
    let mut b = empty_mesh();
    striped_cylinder(&mut a, &mut b, 0.11, 0.09, 0.9, 6, Mat4::identity());
    (a, b)
}

/// A flat marker disc facing +y, unit radius; scaled/tinted at use sites.
pub fn marker_disc() -> CpuMesh {
    let mut m = empty_mesh();
    let (c, t) = cylinder_y(1.0, 1.0, 0.02);
    append(&mut m, c, t);
    m
}
