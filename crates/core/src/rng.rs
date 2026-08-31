//! Deterministic PCG32 RNG. Hand-rolled (no `rand` dependency) so the exact
//! same sequence is produced on every platform, including wasm32 — this is
//! what guarantees all players generate the identical world from a seed.

#[derive(Clone)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng { state: 0, inc: (seed << 1) | 1 };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform f32 in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform f32 in [a, b).
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + self.f32() * (b - a)
    }

    /// Uniform integer in [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        (self.f32() * n as f32) as usize % n.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn seeds_differ() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let same = (0..100).filter(|_| a.next_u32() == b.next_u32()).count();
        assert!(same < 5);
    }

    #[test]
    fn f32_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let x = r.f32();
            assert!((0.0..1.0).contains(&x));
        }
    }
}
