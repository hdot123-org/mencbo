//! IPC Watchdog for detecting webview/main thread hangs (M4 diagnostics).
//!
//! Architecture decisions:
//! - Decision 5: heartbeat_ping is async fn command
//! - Decision 10: MENCBO_DIAG_TEST env var for fault injection
//!
//! State machine: Healthy → Unresponsive → Recovered
//! - Only emits events on state transitions (not repeated)
//! - Visibility-aware: only checks heartbeat when panel is visible
//! - Uses Instant for timing (handles sleep/wake correctly)
//! - 5s check period, 45s threshold, 2 consecutive timeouts required

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Heartbeat timeout threshold (45 seconds)
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);

/// Watchdog check interval (5 seconds)
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Consecutive timeouts required before declaring unresponsive
const CONSECUTIVE_TIMEOUTS_REQUIRED: u32 = 2;

/// Watchdog state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogState {
    Healthy,
    Unresponsive,
    Recovered,
}

/// State transition event (emitted to analytics)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogTransition {
    Unresponsive {
        missed_beats: u64,
        threshold_s: u64,
    },
    Recovered {
        freeze_duration_ms: u64,
    },
}

/// Watchdog for detecting webview/main thread hangs
pub struct Watchdog {
    /// Last heartbeat timestamp (monotonic)
    last_heartbeat: AtomicU64,
    /// Panel visibility state
    is_visible: AtomicBool,
    /// Current state
    state: std::sync::Mutex<WatchdogState>,
    /// When unresponsive state started (for freeze_duration_ms)
    unresponsive_since: std::sync::Mutex<Option<Instant>>,
    /// Consecutive timeout count
    consecutive_timeouts: std::sync::Mutex<u32>,
    /// Last time we checked (for detecting sleep/wake)
    last_check: std::sync::Mutex<Instant>,
}

static WATCHDOG: OnceLock<Watchdog> = OnceLock::new();

impl Watchdog {
    /// Create a new watchdog instance
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_heartbeat: AtomicU64::new(now.elapsed().as_millis() as u64),
            is_visible: AtomicBool::new(false),
            state: std::sync::Mutex::new(WatchdogState::Healthy),
            unresponsive_since: std::sync::Mutex::new(None),
            consecutive_timeouts: std::sync::Mutex::new(0),
            last_check: std::sync::Mutex::new(now),
        }
    }

    /// Get or initialize the global watchdog instance
    pub fn global() -> &'static Watchdog {
        WATCHDOG.get_or_init(|| Watchdog::new())
    }

    /// Initialize the last heartbeat to current time
    /// Should be called when the watchdog starts monitoring
    pub fn set_last_heartbeat(&self, age: Duration) {
        let now = Instant::now();
        let beat_time = now.checked_sub(age).unwrap_or(now);
        let elapsed = now.duration_since(beat_time);
        self.last_heartbeat
            .store(elapsed.as_millis() as u64, Ordering::SeqCst);
    }

    /// Record a heartbeat from the webview (called by heartbeat_ping command)
    pub fn record_heartbeat(&self) {
        let now = Instant::now();
        self.last_heartbeat
            .store(now.elapsed().as_millis() as u64, Ordering::SeqCst);
    }

    /// Update panel visibility state
    pub fn set_visibility(&self, visible: bool) {
        self.is_visible.store(visible, Ordering::SeqCst);
    }

    /// Check if the webview is responsive
    /// Returns Some(state_change) if state changed, None otherwise
    pub fn check(&self) -> Option<WatchdogStateChange> {
        let now = Instant::now();

        // Check for sleep/wake: if time since last check is much larger than expected,
        // the system likely slept. Reset state to avoid false positives.
        {
            let mut last_check = self.last_check.lock().unwrap();
            let elapsed = now.duration_since(*last_check);
            *last_check = now;

            // If elapsed is more than 2x the check interval, we likely slept
            if elapsed > CHECK_INTERVAL * 2 {
                // Reset state after sleep/wake
                let mut state = self.state.lock().unwrap();
                *state = WatchdogState::Healthy;
                *self.consecutive_timeouts.lock().unwrap() = 0;
                *self.unresponsive_since.lock().unwrap() = None;
                return None;
            }
        }

        // Only check heartbeat when panel is visible
        if !self.is_visible.load(Ordering::SeqCst) {
            // Reset consecutive timeouts when hidden (no false positives after show)
            *self.consecutive_timeouts.lock().unwrap() = 0;
            return None;
        }

        // Calculate time since last heartbeat
        let last_beat_ms = self.last_heartbeat.load(Ordering::SeqCst);
        let now_ms = now.elapsed().as_millis() as u64;
        let time_since_beat = Duration::from_millis(now_ms.saturating_sub(last_beat_ms));

        // Check if heartbeat is stale
        if time_since_beat > HEARTBEAT_TIMEOUT {
            let mut timeouts = self.consecutive_timeouts.lock().unwrap();
            *timeouts += 1;

            // Need 2 consecutive timeouts before declaring unresponsive
            if *timeouts >= CONSECUTIVE_TIMEOUTS_REQUIRED {
                let mut state = self.state.lock().unwrap();
                if *state == WatchdogState::Healthy {
                    // Transition to Unresponsive
                    *state = WatchdogState::Unresponsive;
                    *self.unresponsive_since.lock().unwrap() = Some(now);
                    let missed_beats = time_since_beat.as_secs() / CHECK_INTERVAL.as_secs();
                    return Some(WatchdogStateChange::BecameUnresponsive {
                        missed_beats,
                        threshold_s: HEARTBEAT_TIMEOUT.as_secs(),
                    });
                }
            }
        } else {
            // Heartbeat is healthy
            let mut state = self.state.lock().unwrap();
            if *state == WatchdogState::Unresponsive {
                // Transition to Recovered
                let freeze_duration_ms = self
                    .unresponsive_since
                    .lock()
                    .unwrap()
                    .map(|since| now.duration_since(since).as_millis() as u64)
                    .unwrap_or(0);
                *state = WatchdogState::Recovered;
                *self.unresponsive_since.lock().unwrap() = None;
                *self.consecutive_timeouts.lock().unwrap() = 0;
                return Some(WatchdogStateChange::Recovered { freeze_duration_ms });
            } else if *state == WatchdogState::Recovered {
                // Transition back to Healthy
                *state = WatchdogState::Healthy;
            }
            // Reset timeout counter
            *self.consecutive_timeouts.lock().unwrap() = 0;
        }

        None
    }

    /// Check and return a transition event if state changed
    /// This is the public API used by the watchdog thread
    pub fn check_and_transition(&self) -> Option<WatchdogTransition> {
        match self.check() {
            Some(WatchdogStateChange::BecameUnresponsive { missed_beats, threshold_s }) => {
                Some(WatchdogTransition::Unresponsive { missed_beats, threshold_s })
            }
            Some(WatchdogStateChange::Recovered { freeze_duration_ms }) => {
                Some(WatchdogTransition::Recovered { freeze_duration_ms })
            }
            None => None,
        }
    }

    /// Start the monitoring loop (called from a dedicated thread)
    /// Calls the provided callback function when state transitions occur
    pub fn start_monitoring_loop<F>(&self, on_transition: F)
    where
        F: Fn(WatchdogTransition) + Send + 'static,
    {
        // Initialize last heartbeat to current time
        self.set_last_heartbeat(Duration::ZERO);

        loop {
            std::thread::sleep(CHECK_INTERVAL);

            if let Some(transition) = self.check_and_transition() {
                on_transition(transition);
            }
        }
    }

    /// Get current state (for testing)
    #[cfg(test)]
    pub fn get_state(&self) -> WatchdogState {
        *self.state.lock().unwrap()
    }

    /// Manually set state (for testing)
    #[cfg(test)]
    pub fn set_state(&self, new_state: WatchdogState) {
        *self.state.lock().unwrap() = new_state;
    }

    /// Get consecutive timeout count (for testing)
    #[cfg(test)]
    pub fn get_consecutive_timeouts(&self) -> u32 {
        *self.consecutive_timeouts.lock().unwrap()
    }
}

impl Default for Watchdog {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents a state change in the watchdog
#[derive(Debug, Clone, PartialEq)]
pub enum WatchdogStateChange {
    /// Transitioned from Healthy to Unresponsive
    BecameUnresponsive {
        missed_beats: u64,
        threshold_s: u64,
    },
    /// Transitioned from Unresponsive to Recovered
    Recovered { freeze_duration_ms: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_starts_healthy() {
        let wd = Watchdog::new();
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    #[test]
    fn heartbeat_keeps_healthy_state() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.record_heartbeat();
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    #[test]
    fn single_timeout_stays_healthy() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
        assert_eq!(wd.get_consecutive_timeouts(), 1);
    }

    #[test]
    fn two_consecutive_timeouts_become_unresponsive() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));

        // First timeout
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
        assert_eq!(wd.get_consecutive_timeouts(), 1);

        // Second timeout - should transition to Unresponsive
        let change = wd.check();
        assert!(change.is_some());
        assert_eq!(
            change.unwrap(),
            WatchdogStateChange::BecameUnresponsive {
                missed_beats: 9, // 45s / 5s = 9
                threshold_s: 45,
            }
        );
        assert_eq!(wd.get_state(), WatchdogState::Unresponsive);
    }

    #[test]
    fn heartbeat_during_unresponsive_recovers() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));

        // Become unresponsive
        wd.check(); // First timeout
        wd.check(); // Second timeout - becomes unresponsive
        assert_eq!(wd.get_state(), WatchdogState::Unresponsive);

        // Record new heartbeat
        wd.record_heartbeat();

        // Should recover
        let change = wd.check();
        assert!(change.is_some());
        match change.unwrap() {
            WatchdogStateChange::Recovered { freeze_duration_ms } => {
                assert!(freeze_duration_ms >= 0);
            }
            _ => panic!("Expected Recovered state change"),
        }
        assert_eq!(wd.get_state(), WatchdogState::Recovered);
    }

    #[test]
    fn recovered_returns_to_healthy_on_next_check() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_state(WatchdogState::Recovered);

        // Record heartbeat to stay healthy
        wd.record_heartbeat();

        // Should transition from Recovered to Healthy
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    #[test]
    fn hidden_panel_skips_check() {
        let wd = Watchdog::new();
        wd.set_visibility(false);
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(100));

        // Should not check when hidden
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    #[test]
    fn hidden_panel_resets_timeout_counter() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));

        // One timeout while visible
        wd.check();
        assert_eq!(wd.get_consecutive_timeouts(), 1);

        // Hide panel
        wd.set_visibility(false);
        wd.check();
        assert_eq!(wd.get_consecutive_timeouts(), 0);
    }

    #[test]
    fn sleep_wake_resets_state() {
        let wd = Watchdog::new();
        wd.set_visibility(true);

        // Manually set last_check to far in the past to simulate sleep
        {
            let mut last_check = wd.last_check.lock().unwrap();
            *last_check = Instant::now() - Duration::from_secs(60);
        }

        // Set unresponsive state
        wd.set_state(WatchdogState::Unresponsive);

        // Check should detect sleep/wake and reset
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }
}
