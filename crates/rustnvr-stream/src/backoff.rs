//! Exponential backoff with jitter for reconnection.

use std::time::Duration;

/// Backoff parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackoffConfig {
    /// First delay.
    pub initial: Duration,
    /// Maximum delay.
    pub max: Duration,
    /// A session lasting at least this long resets the delay to `initial`.
    pub stable_after: Duration,
}

impl From<&rustnvr_core::config::ReconnectConfig> for BackoffConfig {
    fn from(c: &rustnvr_core::config::ReconnectConfig) -> Self {
        Self {
            initial: Duration::from_millis(c.initial_millis.into()),
            max: Duration::from_secs(c.max_seconds.into()),
            stable_after: Duration::from_secs(c.stable_after_seconds.into()),
        }
    }
}

/// Doubling delay, capped at `max`, with up to 20 % negative jitter so that
/// cameras behind the same failing switch do not reconnect in lockstep.
#[derive(Debug)]
pub struct Backoff {
    config: BackoffConfig,
    next: Duration,
    rng: u64,
}

impl Backoff {
    /// Creates a backoff seeded from the clock.
    pub fn new(config: BackoffConfig) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9_7f4a_7c15, |d| d.as_nanos() as u64);
        Self::with_seed(config, seed)
    }

    /// Creates a backoff with a fixed jitter seed (for tests).
    pub fn with_seed(config: BackoffConfig, seed: u64) -> Self {
        Self {
            config,
            next: config.initial,
            rng: seed | 1,
        }
    }

    /// Returns the delay to wait now and increases the following one.
    pub fn next_delay(&mut self) -> Duration {
        let base = self.next.min(self.config.max);
        self.next = base.saturating_mul(2).min(self.config.max);
        // xorshift64: quality is irrelevant, only spreading matters.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let jitter = (self.rng % 1000) as f64 / 1000.0 * 0.2;
        base.mul_f64(1.0 - jitter)
    }

    /// Resets the delay after a stable session.
    pub fn reset(&mut self) {
        self.next = self.config.initial;
    }

    /// Configuration.
    pub fn config(&self) -> &BackoffConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BackoffConfig {
        BackoffConfig {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(8),
            stable_after: Duration::from_secs(30),
        }
    }

    #[test]
    fn doubles_until_max_with_bounded_jitter() {
        let mut b = Backoff::with_seed(cfg(), 42);
        let expected = [1, 2, 4, 8, 8, 8];
        for secs in expected {
            let d = b.next_delay();
            let nominal = Duration::from_secs(secs);
            assert!(
                d <= nominal && d >= nominal.mul_f64(0.8),
                "{d:?} vs {nominal:?}"
            );
        }
        b.reset();
        assert!(b.next_delay() <= Duration::from_secs(1));
    }
}
