use rand::SeedableRng;
use rand_distr::{Distribution, LogNormal, Poisson};
use rand_pcg::Pcg64;

pub struct RetailOrder {
    pub is_buy: bool,
    pub size: f64,
}

pub struct RetailTrader {
    buy_prob: f64,
    rng: Pcg64,
    poisson: Poisson<f64>,
    lognormal: LogNormal<f64>,
}

impl RetailTrader {
    pub fn new(
        arrival_rate: f64,
        mean_size: f64,
        size_sigma: f64,
        buy_prob: f64,
        seed: u64,
    ) -> Self {
        let sigma = size_sigma.max(0.01);
        let mu_ln = mean_size.max(0.01).ln() - 0.5 * sigma * sigma;
        Self {
            buy_prob,
            rng: Pcg64::seed_from_u64(seed),
            poisson: Poisson::new(arrival_rate.max(0.01)).unwrap(),
            lognormal: LogNormal::new(mu_ln, sigma).unwrap(),
        }
    }

    #[inline]
    pub fn generate_orders(&mut self) -> Vec<RetailOrder> {
        let n = self.poisson.sample(&mut self.rng) as usize;
        if n == 0 {
            return Vec::new();
        }
        (0..n)
            .map(|_| {
                let size = self.lognormal.sample(&mut self.rng);
                let is_buy = rand::Rng::gen::<f64>(&mut self.rng) < self.buy_prob;
                RetailOrder { is_buy, size }
            })
            .collect()
    }
}
