//! Browser bindings for waldo-core. The heavy vertex/instance data crosses
//! the wasm boundary as typed arrays (no JSON), metadata as one JSON string.

use js_sys::{Float32Array, Uint32Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WorldHandle {
    world: waldo_core::World,
}

#[wasm_bindgen]
pub fn generate_world(seed: u32) -> WorldHandle {
    WorldHandle { world: waldo_core::generate_world(seed) }
}

/// Client-side score preview (server remains authoritative).
#[wasm_bindgen]
pub fn score_find(found: bool, time_left_frac: f32, misses: u32) -> u32 {
    waldo_core::score_find(found, time_left_frac, misses)
}

/// How close a click must land to Waldo to count (world units).
#[wasm_bindgen]
pub fn waldo_hit_radius() -> f32 {
    waldo_core::WALDO_HIT_RADIUS
}

#[wasm_bindgen]
impl WorldHandle {
    /// `{ seed, shape_name, bounding_radius, waldo: {pos, quat, scale},
    ///    sets: [{kind, count}] }`
    #[wasm_bindgen(js_name = metaJson)]
    pub fn meta_json(&self) -> String {
        serde_json::to_string(&self.world).unwrap()
    }

    pub fn positions(&self) -> Float32Array {
        Float32Array::from(self.world.mesh.positions.as_slice())
    }
    pub fn normals(&self) -> Float32Array {
        Float32Array::from(self.world.mesh.normals.as_slice())
    }
    pub fn uvs(&self) -> Float32Array {
        Float32Array::from(self.world.mesh.uvs.as_slice())
    }
    pub fn indices(&self) -> Uint32Array {
        Uint32Array::from(self.world.mesh.indices.as_slice())
    }

    /// 10 floats per instance: pos(3) quat(4) scale(3).
    #[wasm_bindgen(js_name = setTransforms)]
    pub fn set_transforms(&self, set_index: usize) -> Float32Array {
        Float32Array::from(self.world.sets[set_index].transforms.as_slice())
    }
    /// 3 floats per instance (primary color), may be empty.
    #[wasm_bindgen(js_name = setColors)]
    pub fn set_colors(&self, set_index: usize) -> Float32Array {
        Float32Array::from(self.world.sets[set_index].colors.as_slice())
    }
    /// 3 floats per instance (secondary color, e.g. house walls), may be empty.
    #[wasm_bindgen(js_name = setColors2)]
    pub fn set_colors2(&self, set_index: usize) -> Float32Array {
        Float32Array::from(self.world.sets[set_index].colors2.as_slice())
    }
}
