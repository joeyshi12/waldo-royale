//! Prints world facts for a seed — used to cross-check native vs wasm output.
//! Usage: cargo run -p waldo-core --example print_world -- <seed>

fn main() {
    let seed: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1234);
    let w = waldo_core::generate_world(seed);
    println!("shape: {}", w.shape_name);
    println!("radius: {:.5}", w.bounding_radius);
    println!(
        "waldo: {:.5}, {:.5}, {:.5}",
        w.waldo.pos[0], w.waldo.pos[1], w.waldo.pos[2]
    );
    println!("verts: {}", w.mesh.positions.len() / 3);
    println!("instances: {}", w.sets.iter().map(|s| s.count).sum::<u32>());
}
