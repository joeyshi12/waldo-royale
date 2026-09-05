//! Procedural planet texture, seeded so every player paints the same one.

use three_d::{CpuTexture, TextureData};
use waldo_core::rng::Rng;

pub fn planet_texture(seed: u32) -> CpuTexture {
    const W: usize = 512;
    const H: usize = 256;
    let mut rng = Rng::new(seed as u64 ^ 0x9e37);
    let base = [[78u8, 143, 58], [71, 132, 64], [90, 148, 64], [63, 125, 51]];
    let b = base[rng.below(base.len())];
    let mut px = vec![[b[0], b[1], b[2], 255u8]; W * H];

    let mut blob = |rng: &mut Rng, colors: &[[u8; 3]], n: usize, rx: (f32, f32), ry: (f32, f32)| {
        for _ in 0..n {
            let c = colors[rng.below(colors.len())];
            let cx = rng.f32() * W as f32;
            let cy = rng.f32() * H as f32;
            let ax = rng.range(rx.0, rx.1);
            let ay = rng.range(ry.0, ry.1);
            let (x0, x1) = ((cx - ax).max(0.0) as usize, ((cx + ax) as usize).min(W - 1));
            let (y0, y1) = ((cy - ay).max(0.0) as usize, ((cy + ay) as usize).min(H - 1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let dx = (x as f32 - cx) / ax;
                    let dy = (y as f32 - cy) / ay;
                    if dx * dx + dy * dy <= 1.0 {
                        px[y * W + x] = [c[0], c[1], c[2], 255];
                    }
                }
            }
        }
    };
    let greens = [[69u8, 127, 51], [90, 156, 68], [63, 122, 46], [99, 169, 76], [82, 140, 63]];
    blob(&mut rng, &greens, 500, (4.0, 14.0), (2.0, 8.0));
    let sands = [[201u8, 184, 120], [212, 196, 138], [184, 168, 108]];
    blob(&mut rng, &sands, 8, (10.0, 30.0), (6.0, 14.0));
    let grays = [[141u8, 141, 141], [154, 154, 148], [127, 130, 135]];
    blob(&mut rng, &grays, 20, (12.0, 30.0), (8.0, 16.0));

    CpuTexture {
        data: TextureData::RgbaU8(px),
        width: W as u32,
        height: H as u32,
        ..Default::default()
    }
}
