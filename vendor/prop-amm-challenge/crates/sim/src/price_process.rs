use rand::SeedableRng;
use rand_distr::{Distribution, StandardNormal};
use rand_pcg::Pcg64;

pub struct GBMPriceProcess {
    current_price: f64,
    drift_term: f64,
    vol_term: f64,
    rng: Pcg64,
}

impl GBMPriceProcess {
    pub fn new(initial_price: f64, mu: f64, sigma: f64, dt: f64, seed: u64) -> Self {
        Self {
            current_price: initial_price,
            drift_term: (mu - 0.5 * sigma * sigma) * dt,
            vol_term: sigma * dt.sqrt(),
            rng: Pcg64::seed_from_u64(seed),
        }
    }

    #[inline]
    pub fn current_price(&self) -> f64 {
        self.current_price
    }

    #[inline]
    pub fn step(&mut self) -> f64 {
        let z: f64 = StandardNormal.sample(&mut self.rng);
        self.current_price *= (self.drift_term + self.vol_term * z).exp();
        self.current_price
    }
}
