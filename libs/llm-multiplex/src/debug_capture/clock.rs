//! Keep elapsed, suspend-inclusive, and adjustable wall clocks distinct.

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub(crate) struct CaptureClock {
    pub(crate) started: Instant,
    wall_started: SystemTime,
    continuous_started: Option<Duration>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ClockSnapshot {
    pub(crate) observed_at_unix_ms: u128,
    pub(crate) elapsed_ms: f64,
    pub(crate) wall_elapsed_ms: f64,
    pub(crate) elapsed_including_suspend_ms: Option<f64>,
    pub(crate) suspend_elapsed_ms: Option<f64>,
    pub(crate) wall_clock_adjustment_ms: Option<f64>,
    pub(crate) wall_monotonic_delta_ms: f64,
}

impl CaptureClock {
    pub(crate) fn new() -> Self {
        process_origin();
        Self {
            started: Instant::now(),
            wall_started: SystemTime::now(),
            continuous_started: rua_http::diagnostics::suspend_inclusive_time(),
        }
    }

    pub(crate) fn started_at_unix_ms(&self) -> u128 {
        unix_ms(self.wall_started)
    }

    pub(crate) fn process_clock_id(&self) -> &str {
        &process_origin().1
    }

    pub(crate) fn started_at_process_ms(&self) -> f64 {
        millis(self.started.duration_since(process_origin().0))
    }

    pub(crate) fn snapshot(&self) -> ClockSnapshot {
        let elapsed = self.started.elapsed();
        let wall_now = SystemTime::now();
        let continuous_elapsed = self
            .continuous_started
            .zip(rua_http::diagnostics::suspend_inclusive_time())
            .and_then(|(start, now)| now.checked_sub(start));
        let wall_elapsed_ms = match wall_now.duration_since(self.wall_started) {
            Ok(duration) => millis(duration),
            Err(error) => -millis(error.duration()),
        };
        ClockSnapshot::from_elapsed(
            unix_ms(wall_now),
            elapsed,
            wall_elapsed_ms,
            continuous_elapsed,
        )
    }
}

fn process_origin() -> &'static (Instant, String) {
    static ORIGIN: OnceLock<(Instant, String)> = OnceLock::new();
    ORIGIN.get_or_init(|| (Instant::now(), uuid::Uuid::new_v4().to_string()))
}

impl ClockSnapshot {
    pub(crate) fn from_elapsed(
        observed_at_unix_ms: u128,
        elapsed: Duration,
        wall_elapsed_ms: f64,
        continuous_elapsed: Option<Duration>,
    ) -> Self {
        let elapsed_ms = millis(elapsed);
        let including_suspend = continuous_elapsed.map(millis);
        Self {
            observed_at_unix_ms,
            elapsed_ms,
            wall_elapsed_ms,
            elapsed_including_suspend_ms: including_suspend,
            // Small negative differences can come from reading the clocks in sequence.
            suspend_elapsed_ms: including_suspend.map(|value| (value - elapsed_ms).max(0.0)),
            wall_clock_adjustment_ms: including_suspend.map(|value| wall_elapsed_ms - value),
            wall_monotonic_delta_ms: wall_elapsed_ms - elapsed_ms,
        }
    }
}

fn unix_ms(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
