//! wg-pyrand: CPython-compatible Mersenne Twister.
//!
//! Reproduces the output of CPython's `random.Random` for integer seeds: seeding through
//! `init_by_array`, `random()`, `getrandbits()`, `choice()` (via `_randbelow`) and `choices()`.
//! The generator is deterministic reference output and is not a security boundary.

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;
const INIT_GENRAND_SEED: u32 = 19_650_218;

/// Errors returned by [`Random::choices`], mirroring the `ValueError`/`IndexError` cases of
/// CPython's `random.choices`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoicesError {
    /// Both `weights` and `cum_weights` were given.
    BothWeights,
    /// The population is empty.
    EmptyPopulation,
    /// The number of weights (or cumulative weights) differs from the population size.
    LengthMismatch,
    /// The total of the weights is zero or negative.
    NonPositiveTotal,
    /// The total of the weights is infinite or NaN.
    NonFiniteTotal,
}

/// A Mersenne Twister with CPython's seeding and derived methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Random {
    state: [u32; N],
    index: usize,
}

impl Random {
    /// Seeds the generator as CPython's `random.Random(seed)` does for an `int`.
    ///
    /// The sign is ignored (`abs(seed)` is used), and the magnitude is split into little-endian
    /// 32-bit words that form the `init_by_array` key. Zero uses the one-word key `[0]`.
    #[must_use]
    pub fn new(seed: i128) -> Self {
        let mut magnitude = seed.unsigned_abs();
        let mut key = Vec::with_capacity(4);
        loop {
            key.push(magnitude as u32);
            magnitude >>= 32;
            if magnitude == 0 {
                break;
            }
        }
        Self::from_key(&key)
    }

    /// Seeds the generator from a `init_by_array` key of little-endian 32-bit words.
    /// An empty key is treated as `[0]`.
    #[must_use]
    pub fn from_key(key: &[u32]) -> Self {
        let zero = [0_u32];
        let key = if key.is_empty() { &zero[..] } else { key };
        let mut rng = Self {
            state: [0; N],
            index: N,
        };
        rng.init_genrand(INIT_GENRAND_SEED);
        rng.init_by_array(key);
        rng
    }

    fn init_genrand(&mut self, seed: u32) {
        self.state[0] = seed;
        for i in 1..N {
            let prev = self.state[i - 1];
            self.state[i] = 1_812_433_253_u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(u32::try_from(i).unwrap_or(0));
        }
        self.index = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        let key_len = key.len();
        let mut i = 1_usize;
        let mut j = 0_usize;
        for _ in 0..N.max(key_len) {
            let prev = self.state[i - 1];
            let key_word = key[j];
            self.state[i] = (self.state[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_664_525)))
                .wrapping_add(key_word)
                .wrapping_add(u32::try_from(j).unwrap_or(0));
            i += 1;
            j += 1;
            if i >= N {
                self.state[0] = self.state[N - 1];
                i = 1;
            }
            if j >= key_len {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = self.state[i - 1];
            self.state[i] = (self.state[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_566_083_941)))
                .wrapping_sub(u32::try_from(i).unwrap_or(0));
            i += 1;
            if i >= N {
                self.state[0] = self.state[N - 1];
                i = 1;
            }
        }
        self.state[0] = 0x8000_0000;
        self.index = N;
    }

    fn generate(&mut self) {
        for kk in 0..N - M {
            let y = (self.state[kk] & UPPER_MASK) | (self.state[kk + 1] & LOWER_MASK);
            self.state[kk] = self.state[kk + M] ^ (y >> 1) ^ mag01(y);
        }
        for kk in N - M..N - 1 {
            let y = (self.state[kk] & UPPER_MASK) | (self.state[kk + 1] & LOWER_MASK);
            self.state[kk] = self.state[kk + M - N] ^ (y >> 1) ^ mag01(y);
        }
        let y = (self.state[N - 1] & UPPER_MASK) | (self.state[0] & LOWER_MASK);
        self.state[N - 1] = self.state[M - 1] ^ (y >> 1) ^ mag01(y);
        self.index = 0;
    }

    /// The next tempered 32-bit output word (`genrand_uint32`).
    fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            self.generate();
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `random.random()`: a float in `[0, 1)` with 53 random bits.
    #[must_use]
    pub fn random(&mut self) -> f64 {
        let a = f64::from(self.next_u32() >> 5);
        let b = f64::from(self.next_u32() >> 6);
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `random.getrandbits(k)` for `0 <= k <= 64`. Returns `None` for larger `k`.
    ///
    /// Words are drawn least significant first; the final partial word keeps its top bits.
    #[must_use]
    pub fn getrandbits(&mut self, k: u32) -> Option<u64> {
        if k > 64 {
            return None;
        }
        if k == 0 {
            return Some(0);
        }
        if k <= 32 {
            return Some(u64::from(self.next_u32() >> (32 - k)));
        }
        let mut out = 0_u64;
        let mut remaining = k;
        let mut shift = 0_u32;
        while remaining > 0 {
            let mut word = self.next_u32();
            if remaining < 32 {
                word >>= 32 - remaining;
            }
            out |= u64::from(word) << shift;
            shift += 32;
            remaining = remaining.saturating_sub(32);
        }
        Some(out)
    }

    /// CPython's `_randbelow`: a uniform integer in `[0, n)`. Returns `None` when `n == 0`.
    #[must_use]
    pub fn randbelow(&mut self, n: u64) -> Option<u64> {
        if n == 0 {
            return None;
        }
        let bits = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(bits)?;
            if r < n {
                return Some(r);
            }
        }
    }

    /// `random.choice(seq)`. Returns `None` for an empty sequence.
    #[must_use]
    pub fn choice<'a, T>(&mut self, seq: &'a [T]) -> Option<&'a T> {
        let n = u64::try_from(seq.len()).ok()?;
        let index = self.randbelow(n)?;
        seq.get(usize::try_from(index).ok()?)
    }

    /// `random.choices(population, weights=..., cum_weights=..., k=...)`.
    ///
    /// With neither weight argument every draw is `population[floor(random() * n)]`. With
    /// `weights` the cumulative sums are built first, as CPython does. Draws use bisection over
    /// `cum_weights[..n - 1]`, scaled by the total.
    ///
    /// # Errors
    ///
    /// Returns a [`ChoicesError`] when the arguments are inconsistent, as CPython raises.
    pub fn choices<'a, T>(
        &mut self,
        population: &'a [T],
        weights: Option<&[f64]>,
        cum_weights: Option<&[f64]>,
        k: usize,
    ) -> Result<Vec<&'a T>, ChoicesError> {
        if weights.is_some() && cum_weights.is_some() {
            return Err(ChoicesError::BothWeights);
        }
        let n = population.len();
        if n == 0 {
            return Err(ChoicesError::EmptyPopulation);
        }
        if weights.is_none() && cum_weights.is_none() {
            let scale = n as f64;
            let mut out = Vec::with_capacity(k);
            for _ in 0..k {
                let index = (self.random() * scale).floor() as usize;
                out.push(&population[index.min(n - 1)]);
            }
            return Ok(out);
        }
        let cum: Vec<f64> = match (weights, cum_weights) {
            (Some(w), _) => {
                let mut acc = 0.0_f64;
                w.iter()
                    .map(|&x| {
                        acc += x;
                        acc
                    })
                    .collect()
            }
            (None, Some(c)) => c.to_vec(),
            (None, None) => Vec::new(),
        };
        if cum.len() != n {
            return Err(ChoicesError::LengthMismatch);
        }
        let total = cum[n - 1] + 0.0;
        if total.is_nan() || total.is_infinite() {
            return Err(ChoicesError::NonFiniteTotal);
        }
        if total <= 0.0 {
            return Err(ChoicesError::NonPositiveTotal);
        }
        let hi = n - 1;
        let mut out = Vec::with_capacity(k);
        for _ in 0..k {
            let x = self.random() * total;
            out.push(&population[bisect_right(&cum, x, hi)]);
        }
        Ok(out)
    }
}

fn mag01(y: u32) -> u32 {
    if y & 1 == 0 { 0 } else { MATRIX_A }
}

/// `bisect.bisect_right(a, x, 0, hi)`: the insertion point of `x` within `a[..hi]`.
fn bisect_right(a: &[f64], x: f64, hi: usize) -> usize {
    let mut lo = 0_usize;
    let mut high = hi;
    while lo < high {
        let mid = usize::midpoint(lo, high);
        if x < a[mid] {
            high = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}
