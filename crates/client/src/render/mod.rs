//! The three-d renderer: fixed camera, the planet (and everything on it)
//! rotates. Rebuilds the scene whenever the shared state's round changes.

mod characters;
mod texture;

use crate::net;
use crate::state::{now_ms, Shared, Ui};
use leptos::prelude::*;
use characters::*;
use std::collections::HashMap;
use texture::planet_texture;
use three_d::*;
use waldo_core::World;

const MODEL_KINDS: [(&str, &[&str], f32); 9] = [
    ("pine", &["tree_pineDefaultA", "tree_pineDefaultB", "tree_pineRoundA", "tree_pineRoundC", "tree_pineTallA"], 1.25),
    ("tree", &["tree_default", "tree_default_dark", "tree_oak", "tree_detailed", "tree_fat"], 0.9),
    ("bush", &["plant_bush", "plant_bushLarge", "plant_bushDetailed", "flower_redA", "mushroom_red"], 0.32),
    ("building", &["building-a", "building-b", "building-c", "building-d", "building-e", "building-f", "building-g", "building-h"], 1.05),
    ("tower", &["building-skyscraper-a", "building-skyscraper-b", "building-skyscraper-c", "building-skyscraper-d", "building-skyscraper-e"], 1.15),
    ("house", &["building-type-a", "building-type-b", "building-type-c", "building-type-d", "building-type-e", "building-type-f", "building-type-g", "building-type-h"], 0.9),
    ("dome", &["detail-parasol-a", "detail-parasol-b"], 0.55),
    ("car", &["sedan", "suv", "taxi", "van", "police", "hatchback-sports", "sedan-sports", "truck"], 0.24),
    ("rock", &["rock_largeA", "rock_largeB", "rock_largeC", "rock_tallA", "rock_tallB", "rock_smallA"], 0.4),
];

struct ModelPart {
    tri: CpuMesh,
    material: PhysicalMaterial,
}

struct Model {
    parts: Vec<ModelPart>,
    norm: Mat4,
}

type ModelLib = HashMap<&'static str, Model>;

async fn load_models(context: &Context) -> ModelLib {
    let paths: Vec<String> = MODEL_KINDS
        .iter()
        .flat_map(|(_, names, _)| names.iter())
        .map(|n| format!("assets/models/{n}.glb"))
        .collect();
    let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    let mut assets = three_d_asset::io::load_async(&path_refs).await.expect("load models");

    let mut lib = ModelLib::new();
    for (_, names, _) in MODEL_KINDS.iter() {
        for name in names.iter() {
            let cpu: CpuModel = assets.deserialize(format!("{name}.glb")).expect("glb");
            let mut aabb = AxisAlignedBoundingBox::EMPTY;
            for g in cpu.geometries.iter() {
                aabb.expand_with_aabb(g.compute_aabb());
            }
            let (min, max) = (aabb.min(), aabb.max());
            let h = (max.y - min.y).max(1e-4);
            let norm = Mat4::from_scale(1.0 / h)
                * Mat4::from_translation(vec3(
                    -(min.x + max.x) / 2.0,
                    -min.y,
                    -(min.z + max.z) / 2.0,
                ));
            let mut parts = Vec::new();
            for g in cpu.geometries.iter() {
                let three_d_asset::geometry::Geometry::Triangles(tri) = &g.geometry else {
                    continue;
                };
                let cpu_mat = g
                    .material_index
                    .and_then(|i| cpu.materials.get(i))
                    .cloned()
                    .unwrap_or_default();
                let material = PhysicalMaterial::new_opaque(context, &cpu_mat);
                parts.push(ModelPart { tri: tri.clone(), material });
            }
            lib.insert(name, Model { parts, norm });
        }
    }
    lib
}

fn placement(pos: &[f32; 3], quat: &[f32; 4], scale: &[f32; 3], extra: f32) -> Mat4 {
    Mat4::from_translation(vec3(pos[0], pos[1], pos[2]))
        * Mat4::from(Quat::new(quat[3], quat[0], quat[1], quat[2]))
        * Mat4::from_nonuniform_scale(scale[0] * extra, scale[1] * extra, scale[2] * extra)
}

type PGm = Gm<Mesh, PhysicalMaterial>;
type IGm = Gm<InstancedMesh, PhysicalMaterial>;

struct CharState {
    role: String,
    local: Mat4,
    pos: Vec3, // planet-local base
}

type CharGms = Vec<(String, Vec<(PGm, ())>, Vec<Gm<Mesh, ColorMaterial>>)>;

struct Marker {
    gm: Gm<Mesh, ColorMaterial>,
    pos: Vec3, // planet-local
    born_ms: f64,
    persistent: bool,
}

struct Scene {
    planet: PGm,
    instanced: Vec<(IGm, Mat4)>,
    char_state: Vec<CharState>,
    markers: Vec<Marker>,
    rot: Quat,
    fit_radius: f32,
    revealed: bool,
    night: bool,
}

fn solid_material(context: &Context, color: Srgba) -> PhysicalMaterial {
    PhysicalMaterial::new_opaque(
        context,
        &CpuMaterial { albedo: color, roughness: 1.0, metallic: 0.0, ..Default::default() },
    )
}

fn build_scene(context: &Context, lib: &ModelLib, world: &World, mutator: &str, seed: u32) -> Scene {
    // planet mesh straight from core
    let mesh = &world.mesh;
    let planet_cpu = CpuMesh {
        positions: Positions::F32(
            mesh.positions.chunks_exact(3).map(|c| vec3(c[0], c[1], c[2])).collect(),
        ),
        normals: Some(mesh.normals.chunks_exact(3).map(|c| vec3(c[0], c[1], c[2])).collect()),
        uvs: Some(mesh.uvs.chunks_exact(2).map(|c| vec2(c[0], c[1])).collect()),
        indices: Indices::U32(mesh.indices.clone()),
        ..Default::default()
    };
    let planet = Gm::new(
        Mesh::new(context, &planet_cpu),
        PhysicalMaterial::new_opaque(
            context,
            &CpuMaterial {
                albedo: Srgba::WHITE,
                albedo_texture: Some(planet_texture(seed)),
                roughness: 1.0,
                metallic: 0.0,
                ..Default::default()
            },
        ),
    );

    let mut instanced: Vec<(IGm, Mat4)> = Vec::new();
    for set in &world.sets {
        let t = &set.transforms;
        let mats = |extra: f32| -> Vec<Mat4> {
            (0..set.count as usize)
                .map(|i| {
                    let o = i * 10;
                    placement(
                        &[t[o], t[o + 1], t[o + 2]],
                        &[t[o + 3], t[o + 4], t[o + 5], t[o + 6]],
                        &[t[o + 7], t[o + 8], t[o + 9]],
                        extra,
                    )
                })
                .collect()
        };
        if let Some((_, variants, size)) = MODEL_KINDS.iter().find(|(k, _, _)| *k == set.kind) {
            let all = mats(*size);
            let mut by_variant: Vec<Vec<Mat4>> = vec![Vec::new(); variants.len()];
            for (i, m) in all.into_iter().enumerate() {
                by_variant[i % variants.len()].push(m);
            }
            for (v, name) in variants.iter().enumerate() {
                if by_variant[v].is_empty() {
                    continue;
                }
                let model = &lib[name];
                let instances = Instances {
                    transformations: by_variant[v].iter().map(|m| m * model.norm).collect(),
                    ..Default::default()
                };
                for part in &model.parts {
                    instanced.push((
                        Gm::new(
                            InstancedMesh::new(context, &instances, &part.tri),
                            part.material.clone(),
                        ),
                        Mat4::identity(),
                    ));
                }
            }
        } else if set.kind == "pond" {
            let (mesh, color) = pond();
            let instances = Instances { transformations: mats(1.0), ..Default::default() };
            instanced.push((
                Gm::new(InstancedMesh::new(context, &instances, &mesh), solid_material(context, color)),
                Mat4::identity(),
            ));
        } else if set.kind == "decoy" {
            let (a, b) = decoy();
            let instances = Instances { transformations: mats(1.0), ..Default::default() };
            instanced.push((
                Gm::new(InstancedMesh::new(context, &instances, &a), solid_material(context, RED)),
                Mat4::identity(),
            ));
            instanced.push((
                Gm::new(InstancedMesh::new(context, &instances, &b), solid_material(context, WHITE)),
                Mat4::identity(),
            ));
        }
    }

    // characters are persistent GPU objects; the scene only carries their
    // per-round placements
    let mut char_state = Vec::new();
    let mut add_char = |role: &str, pos: &[f32; 3], quat: &[f32; 4], scale: &[f32; 3]| {
        char_state.push(CharState {
            role: role.to_string(),
            local: placement(pos, quat, scale, 1.0),
            pos: vec3(pos[0], pos[1], pos[2]),
        });
    };
    add_char("waldo", &world.waldo.pos, &world.waldo.quat, &world.waldo.scale);
    for c in &world.cast {
        add_char(&c.role, &c.pos, &c.quat, &c.scale);
    }

    // fair spawn: seeded, Waldo on the far side of the planet
    let mut spawn_rng = waldo_core::rng::Rng::new(seed as u64 ^ 0x51ab);
    let waldo_dir = vec3(world.waldo.pos[0], world.waldo.pos[1], world.waldo.pos[2]).normalize();
    let cam_dir = vec3(0.0, 9.0, 30.0).normalize();
    let away = -cam_dir;
    let align = Quat::between_vectors(waldo_dir, away);
    let roll = Quat::from_axis_angle(away, Rad(spawn_rng.range(0.0, std::f32::consts::TAU)));
    let rot = roll * align;

    Scene {
        planet,
        instanced,
        char_state,
        markers: Vec::new(),
        rot,
        fit_radius: world.bounding_radius + 2.8,
        revealed: false,
        night: mutator == "night",
    }
}

fn apply_rotation(scene: &mut Scene, char_gms: &mut CharGms) {
    let r = Mat4::from(scene.rot);
    scene.planet.set_transformation(r);
    for (gm, local) in scene.instanced.iter_mut() {
        gm.set_transformation(r * *local);
    }
    for st in scene.char_state.iter() {
        if let Some((_, parts, outline)) = char_gms.iter_mut().find(|(role, _, _)| *role == st.role) {
            for (gm, _) in parts.iter_mut() {
                gm.set_transformation(r * st.local);
            }
            let o = if scene.revealed { r * st.local * Mat4::from_scale(1.15) } else { Mat4::from_scale(0.0) };
            for gm in outline.iter_mut() {
                gm.set_transformation(o);
            }
        }
    }
    for m in scene.markers.iter_mut() {
        let up = m.pos.normalize();
        let face = Quat::between_vectors(vec3(0.0, 1.0, 0.0), up);
        let age = ((now_ms() - m.born_ms) / 1000.0) as f32;
        let scale = if m.persistent {
            0.9 + 0.2 * (age * 5.0).sin()
        } else {
            0.4 + age * 0.6
        };
        m.gm.set_transformation(
            r * Mat4::from_translation(m.pos + up * 0.06)
                * Mat4::from(face)
                * Mat4::from_scale(scale),
        );
    }
}

pub fn spawn(ui: Ui, game: Shared) {
    wasm_bindgen_futures::spawn_local(run(ui, game));
}

async fn run(ui: Ui, game: Shared) {
    let window = Window::new(WindowSettings {
        title: "waldo".into(),
        ..Default::default()
    })
    .unwrap();
    let context = window.gl();

    // load models in the background so the render loop (and its first navy
    // clear) starts immediately instead of behind seconds of black canvas
    let lib: std::rc::Rc<std::cell::RefCell<Option<ModelLib>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    {
        let lib = lib.clone();
        let context = context.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let loaded = load_models(&context).await;
            *lib.borrow_mut() = Some(loaded);
        });
    }

    // characters are static geometry: build their meshes and materials once
    let char_lib: Vec<(String, Vec<(CpuMesh, Srgba)>)> =
        ["waldo", "wenda", "odlaw", "woof", "wizard"]
            .iter()
            .map(|r| (r.to_string(), build_character(r)))
            .collect();
    let mut solid_cache: std::collections::HashMap<u32, PhysicalMaterial> =
        std::collections::HashMap::new();
    let mut solid = |ctx: &Context, c: Srgba| -> PhysicalMaterial {
        let key = u32::from_be_bytes([c.r, c.g, c.b, c.a]);
        solid_cache.entry(key).or_insert_with(|| solid_material(ctx, c)).clone()
    };
    let char_gms: Vec<(String, Vec<(PGm, ())>, Vec<Gm<Mesh, ColorMaterial>>)> = char_lib
        .iter()
        .map(|(role, parts)| {
            let gms: Vec<(PGm, ())> = parts
                .iter()
                .map(|(mesh, color)| (Gm::new(Mesh::new(&context, mesh), solid(&context, *color)), ()))
                .collect();
            let outline: Vec<Gm<Mesh, ColorMaterial>> = if role == "waldo" {
                parts
                    .iter()
                    .map(|(mesh, _)| {
                        Gm::new(
                            Mesh::new(&context, mesh),
                            ColorMaterial {
                                color: Srgba::WHITE,
                                render_states: RenderStates {
                                    depth_test: DepthTest::Always,
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (role.clone(), gms, outline)
        })
        .collect();

    let ambient = AmbientLight::new(&context, 0.35, Srgba::new(191, 217, 255, 255));
    let mut sun = DirectionalLight::new(&context, 1.4, Srgba::new(255, 242, 217, 255), vec3(-0.6, -0.8, -0.6));
    let mut fill = DirectionalLight::new(&context, 0.3, Srgba::new(136, 153, 255, 255), vec3(0.5, 0.35, 0.35));
    let mut ambient = ambient;
    let mut spot = SpotLight::new(
        &context,
        0.0,
        Srgba::new(255, 242, 208, 255),
        vec3(0.0, 9.0, 30.0),
        vec3(0.0, 0.0, -1.0),
        Rad(0.15),
        Attenuation { constant: 1.0, linear: 0.0, quadratic: 0.0 },
    );

    let cam_dir = vec3(0.0, 9.0, 30.0).normalize();
    let mut camera = Camera::new_perspective(
        window.viewport(),
        cam_dir * 40.0,
        vec3(0.0, 0.0, 0.0),
        vec3(0.0, 1.0, 0.0),
        degrees(45.0),
        0.1,
        500.0,
    );

    // one tiny instanced object to warm the instanced-mesh shader too
    // (created once the model library finishes loading)
    let mut warmup_instanced: Option<IGm> = None;
    let mut char_gms = char_gms;
    // start invisible: warmup renders them at epsilon scale
    for (_, parts, outline) in char_gms.iter_mut() {
        for (gm, _) in parts.iter_mut() {
            gm.set_transformation(Mat4::from_scale(1e-4));
        }
        for gm in outline.iter_mut() {
            gm.set_transformation(Mat4::from_scale(1e-4));
        }
    }

    let mut scene: Option<Scene> = None;
    let mut built_round = 0u32;
    let mut zoom = 1.0f32;
    let mut cursor = PhysicalPoint { x: 0.0, y: 0.0 };
    let mut dragging = false;
    let mut drag_dist = 0.0f32;
    let marker_mesh = marker_disc();

    window.render_loop(move |mut frame_input| {
        camera.set_viewport(frame_input.viewport);

        // ---- (re)build the scene when a round starts ----
        let (round_id, world, mutator, seed) = {
            let g = game.borrow();
            (g.round_id, g.world.clone(), g.mutator.clone(), g.world.as_ref().map(|w| w.seed).unwrap_or(0))
        };
        if warmup_instanced.is_none() {
            if let Some(lib) = lib.borrow().as_ref() {
                warmup_instanced = lib.values().next().map(|m| {
                    let instances = Instances {
                        transformations: vec![Mat4::from_scale(1e-4)],
                        ..Default::default()
                    };
                    Gm::new(
                        InstancedMesh::new(&context, &instances, &m.parts[0].tri),
                        m.parts[0].material.clone(),
                    )
                });
            }
        }

        if round_id != built_round {
            if let (Some(world), Some(lib)) = (world.as_ref(), lib.borrow().as_ref()) {
                let t0 = now_ms();
                let mut s = build_scene(&context, lib, world, &mutator, seed);
                web_sys::console::debug_1(&format!("scene build: {:.0} ms", now_ms() - t0).into());
                zoom = 1.0;
                apply_rotation(&mut s, &mut char_gms);
                let night = s.night;
                ambient.intensity = if night { 0.03 } else { 0.35 };
                sun.intensity = if night { 0.02 } else { 1.4 };
                fill.intensity = if night { 0.0 } else { 0.3 };
                spot.intensity = if night { 1.3 } else { 0.0 };
                scene = Some(s);
                built_round = round_id;
            }
        }

        let Some(s) = scene.as_mut() else {
            // warm the shaders while the menu is up: draw every material kind
            // at epsilon scale so first-round start doesn't stall on compiles
            let mut warm: Vec<&dyn Object> = Vec::new();
            for (_, parts, outline) in char_gms.iter() {
                for (gm, _) in parts.iter().take(1) {
                    warm.push(gm);
                }
                for gm in outline.iter().take(1) {
                    warm.push(gm);
                }
            }
            if let Some(w) = warmup_instanced.as_ref() {
                warm.push(w);
            }
            frame_input
                .screen()
                .clear(ClearState::color_and_depth(0.043, 0.063, 0.15, 1.0, 1.0))
                .render(&camera, warm, &[&ambient, &sun, &fill, &spot]);
            return FrameOutput::default();
        };

        // ---- camera fit (viewport-aware, like the JS client) ----
        let vfov = 45.0f32.to_radians();
        let aspect = frame_input.viewport.width as f32 / frame_input.viewport.height.max(1) as f32;
        let hfov = 2.0 * ((vfov / 2.0).tan() * aspect).atan();
        let base_dist = s.fit_radius / (vfov.min(hfov) / 2.0).tan();
        camera.set_view(cam_dir * (base_dist / zoom), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0));

        // ---- input ----
        let mut rot_changed = false;
        let mut clicked: Option<PhysicalPoint> = None;
        for event in frame_input.events.iter() {
            match event {
                Event::MousePress { button: MouseButton::Left, position, .. } => {
                    dragging = true;
                    drag_dist = 0.0;
                    cursor = *position;
                }
                Event::MouseRelease { button: MouseButton::Left, position, .. } => {
                    if dragging && drag_dist < 4.0 {
                        clicked = Some(*position);
                    }
                    dragging = false;
                }
                Event::MouseMotion { position, delta, button, .. } => {
                    cursor = *position;
                    if dragging && *button == Some(MouseButton::Left) {
                        drag_dist += delta.0.abs() + delta.1.abs();
                        if drag_dist > 4.0 {
                            let speed = 0.006 / zoom.sqrt();
                            let yaw = Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), Rad(delta.0 * speed));
                            let pitch = Quat::from_axis_angle(vec3(1.0, 0.0, 0.0), Rad(delta.1 * speed));
                            s.rot = pitch * yaw * s.rot;
                            rot_changed = true;
                        }
                    }
                }
                Event::MouseWheel { delta, position, .. } => {
                    let before = zoom;
                    zoom = (zoom * (delta.1 * 0.0035).exp()).clamp(1.0, 5.0);
                    if zoom > before {
                        // zoom toward the cursor: drift the picked point to center
                        if let Ok(Some(hit)) =
                            pick(&context, &camera, *position, &s.planet, Cull::Back)
                        {
                            let dir = hit.position.normalize();
                            let full = Quat::between_vectors(dir, cam_dir);
                            let t = (1.0 - before / zoom).min(0.35);
                            let partial = Quat::one().slerp(full, t);
                            s.rot = partial * s.rot;
                            rot_changed = true;
                        }
                    }
                    cursor = *position;
                }
                _ => {}
            }
        }

        // ---- click resolution ----
        if let Some(pos) = clicked {
            if net::can_click(&game) {
                // characters first (glancing clicks must resolve to them)
                let mut char_geoms: Vec<&Gm<Mesh, PhysicalMaterial>> = Vec::new();
                let mut owner: Vec<usize> = Vec::new();
                for (ci, st) in s.char_state.iter().enumerate() {
                    if let Some((_, parts, _)) = char_gms.iter().find(|(r, _, _)| *r == st.role) {
                        for (gm, _) in parts.iter() {
                            char_geoms.push(gm);
                            owner.push(ci);
                        }
                    }
                }
                let char_hit = pick(&context, &camera, pos, char_geoms, Cull::Back)
                    .ok()
                    .flatten();
                let planet_hit = pick(&context, &camera, pos, &s.planet, Cull::Back)
                    .ok()
                    .flatten();
                let inv = Mat4::from(s.rot).invert().unwrap();
                let to_local = |p: Vec3| (inv * p.extend(1.0)).truncate();

                let local = match (char_hit, planet_hit) {
                    (Some(c), Some(p)) => {
                        let cd = (c.position - camera.position()).magnitude();
                        let pd = (p.position - camera.position()).magnitude();
                        if cd <= pd + 0.5 {
                            Some(s.char_state[owner[c.geometry_id as usize]].pos)
                        } else {
                            Some(to_local(p.position))
                        }
                    }
                    (Some(c), None) => Some(s.char_state[owner[c.geometry_id as usize]].pos),
                    (None, Some(p)) => Some(to_local(p.position)),
                    (None, None) => None,
                };
                if let Some(p) = local {
                    net::send_click(&game, [p.x, p.y, p.z]);
                }
            }
        }

        // ---- state-driven effects ----
        {
            let mut g = game.borrow_mut();
            let found_now = g.found;
            // pings from click results
            for (pos, color) in g.pings.drain(..) {
                let gold = color == [255, 225, 74];
                let is_waldo_found = found_now && gold;
                s.markers.push(Marker {
                    gm: Gm::new(
                        Mesh::new(&context, &marker_mesh),
                        ColorMaterial {
                            color: Srgba::new(color[0], color[1], color[2], 255),
                            ..Default::default()
                        },
                    ),
                    pos: vec3(pos[0], pos[1], pos[2]),
                    born_ms: now_ms(),
                    persistent: is_waldo_found,
                });
                rot_changed = true;
            }
            // reveal at round end
            if let Some(w) = g.reveal.take() {
                s.revealed = true;
                let waldo_local = vec3(w[0], w[1], w[2]).normalize();
                let current = Mat4::from(s.rot).transform_vector(waldo_local).normalize();
                s.rot = Quat::between_vectors(current, cam_dir) * s.rot;
                s.markers.push(Marker {
                    gm: Gm::new(
                        Mesh::new(&context, &marker_mesh),
                        ColorMaterial {
                            color: Srgba::new(255, 225, 74, 255),
                            ..Default::default()
                        },
                    ),
                    pos: vec3(w[0], w[1], w[2]),
                    born_ms: now_ms(),
                    persistent: true,
                });
                rot_changed = true;
            }
            // HUD timer
            if g.playing {
                ui.time_left_ms.set((g.ends_at_ms - now_ms()).max(0.0) as i64);
            }
        }

        // expire transient markers
        let n_before = s.markers.len();
        s.markers.retain(|m| m.persistent || now_ms() - m.born_ms < 1000.0);
        if s.markers.len() != n_before {
            rot_changed = true;
        }
        if !s.markers.is_empty() {
            rot_changed = true; // markers animate every frame
        }

        if rot_changed {
            apply_rotation(s, &mut char_gms);
        }

        // ---- night searchlight follows the cursor ----
        if s.night {
            spot.position = camera.position();
            let target = pick(&context, &camera, cursor, &s.planet, Cull::Back)
                .ok()
                .flatten()
                .map(|h| h.position)
                .unwrap_or(vec3(0.0, 0.0, 0.0));
            spot.direction = (target - camera.position()).normalize();
        }

        // ---- render ----
        let mut objects: Vec<&dyn Object> = vec![&s.planet];
        for (gm, _) in s.instanced.iter() {
            objects.push(gm);
        }
        for (_, parts, outline) in char_gms.iter() {
            for (gm, _) in parts.iter() {
                objects.push(gm);
            }
            if s.revealed {
                for gm in outline.iter() {
                    objects.push(gm);
                }
            }
        }
        for m in s.markers.iter() {
            objects.push(&m.gm);
        }

        frame_input
            .screen()
            .clear(ClearState::color_and_depth(0.043, 0.063, 0.15, 1.0, 1.0))
            .render(&camera, objects, &[&ambient, &sun, &fill, &spot]);
        FrameOutput::default()
    });
}
