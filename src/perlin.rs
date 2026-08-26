pub struct PerlinNoise {
    permutation: [u8; 512],
}

impl PerlinNoise {
    pub fn new(seed: u32) -> Self {
        let mut values = [0_u8; 256];
        for (index, value) in values.iter_mut().enumerate() {
            *value = index as u8;
        }

        let mut state = if seed == 0 { 0xA3C5_9AC3 } else { seed };
        for index in (1..values.len()).rev() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let target = state as usize % (index + 1);
            values.swap(index, target);
        }

        let mut permutation = [0_u8; 512];
        for index in 0..permutation.len() {
            permutation[index] = values[index & 255];
        }

        Self { permutation }
    }

    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let cell_x = x.floor() as i32 & 255;
        let cell_y = y.floor() as i32 & 255;
        let local_x = x - x.floor();
        let local_y = y - y.floor();
        let u = fade(local_x);
        let v = fade(local_y);

        let aa = self.hash(cell_x, cell_y);
        let ab = self.hash(cell_x, cell_y + 1);
        let ba = self.hash(cell_x + 1, cell_y);
        let bb = self.hash(cell_x + 1, cell_y + 1);

        let lower = lerp(
            gradient(aa, local_x, local_y),
            gradient(ba, local_x - 1.0, local_y),
            u,
        );
        let upper = lerp(
            gradient(ab, local_x, local_y - 1.0),
            gradient(bb, local_x - 1.0, local_y - 1.0),
            u,
        );
        lerp(lower, upper, v)
    }

    pub fn octave_sample(&self, x: f32, y: f32, octaves: u32) -> f32 {
        let mut value = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut total_amplitude = 0.0;

        for _ in 0..octaves {
            value += self.sample(x * frequency, y * frequency) * amplitude;
            total_amplitude += amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }

        value / total_amplitude
    }

    /// Folding the signal around zero turns rounded domes into sharp crests,
    /// which is what makes mountains look like mountains. Returns 0..=1.
    pub fn ridge_sample(&self, x: f32, y: f32, octaves: u32) -> f32 {
        let mut value = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut total_amplitude = 0.0;

        for _ in 0..octaves {
            let ridge = 1.0 - self.sample(x * frequency, y * frequency).abs();
            value += ridge * ridge * amplitude;
            total_amplitude += amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }

        (value / total_amplitude).clamp(0.0, 1.0)
    }

    fn hash(&self, x: i32, y: i32) -> u8 {
        let first = self.permutation[x as usize & 255] as usize;
        self.permutation[first.wrapping_add(y as usize) & 255]
    }
}

fn fade(value: f32) -> f32 {
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

fn lerp(start: f32, end: f32, amount: f32) -> f32 {
    start + amount * (end - start)
}

fn gradient(hash: u8, x: f32, y: f32) -> f32 {
    match hash & 3 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        _ => -x - y,
    }
}

#[cfg(test)]
mod tests {
    use super::PerlinNoise;

    #[test]
    fn same_seed_produces_the_same_noise() {
        let first = PerlinNoise::new(42).octave_sample(1.25, 9.5, 4);
        let second = PerlinNoise::new(42).octave_sample(1.25, 9.5, 4);
        assert_eq!(first, second);
    }

    #[test]
    fn zero_and_one_are_distinct_seeds() {
        let zero = PerlinNoise::new(0).octave_sample(1.25, 9.5, 4);
        let one = PerlinNoise::new(1).octave_sample(1.25, 9.5, 4);
        assert_ne!(zero, one);
    }

    #[test]
    fn noise_stays_in_expected_range() {
        let noise = PerlinNoise::new(7);
        for x in 0..50 {
            let value = noise.octave_sample(x as f32 * 0.17, 3.4, 5);
            assert!((-1.0..=1.0).contains(&value));
        }
    }
}
