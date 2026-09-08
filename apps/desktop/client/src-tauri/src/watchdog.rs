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
//!
//! Main thread probe:
//! - Distinguishes webview hangs from main thread hangs
//! - Uses run_on_main_thread to execute a closure that updates a timestamp
//! - If timestamp is stale, main thread is unresponsive

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Heartbeat timeout threshold (45 seconds)
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);

/// Watchdog check interval (5 seconds)
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Consecutive timeouts required before declaring unresponsive
const CONSECUTIVE_TIMEOUTS_REQUIRED: u32 = 2;

/// Main thread probe timeout threshold (10 seconds)
/// If main thread hasn't responded within this time, it's considered unresponsive
const MAIN_THREAD_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Watchdog state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogState {
    Healthy,
    Unresponsive,
    Recovered,
}

/// State transition event (emitted to analytics)
#[derive(Debug, Clone, PartialEq)]
pub enum WatchdogTransition {
    /// Webview is unresponsive (JS heartbeat missing)
    Unresponsive {
        missed_beats: u64,
        threshold_s: u64,
    },
    /// Webview recovered from unresponsive state
    Recovered {
        freeze_duration_ms: u64,
    },
    /// Main thread is unresponsive (probe callback not responding)
    /// This takes priority over webview unresponsive (VAL-DIAG-004)
    MainThreadUnresponsive {
        stalled_ms: u64,
    },
}

/// Represents a state change in the watchdog (internal to the module)
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

/// Watchdog for detecting webview/main thread hangs
pub struct Watchdog {
    /// Last heartbeat instant (monotonic, handles sleep/wake correctly)
    last_heartbeat: Mutex<Instant>,
    /// Panel visibility state
    is_visible: AtomicBool,
    /// Current state
    state: Mutex<WatchdogState>,
    /// When unresponsive state started (for freeze_duration_ms)
    unresponsive_since: Mutex<Option<Instant>>,
    /// Consecutive timeout count
    consecutive_timeouts: Mutex<u32>,
    /// Last time we checked (for detecting sleep/wake)
    last_check: Mutex<Instant>,
    /// Last main thread probe response time (for detecting main thread hangs)
    main_thread_probe_time: AtomicU64,
    /// Track if we've already reported main thread unresponsive (one-shot)
    main_thread_unresponsive_reported: AtomicBool,
}

static WATCHDOG: OnceLock<Watchdog> = OnceLock::new();

impl Watchdog {
    /// Create a new watchdog instance
    pub fn new() -> Self {
        let now = Instant::now();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        Self {
            last_heartbeat: Mutex::new(now),
            is_visible: AtomicBool::new(false),
            state: Mutex::new(WatchdogState::Healthy),
            unresponsive_since: Mutex::new(None),
            consecutive_timeouts: Mutex::new(0),
            last_check: Mutex::new(now),
            main_thread_probe_time: AtomicU64::new(now_ms),
            main_thread_unresponsive_reported: AtomicBool::new(false),
        }
    }

    /// Get or initialize the global watchdog instance
    pub fn global() -> &'static Watchdog {
        WATCHDOG.get_or_init(|| Watchdog::new())
    }

    /// Set the last heartbeat to `age` in the past.
    /// Used by tests and by the monitoring loop initialization.
    pub fn set_last_heartbeat(&self, age: Duration) {
        let now = Instant::now();
        let beat_time = now.checked_sub(age).unwrap_or(now);
        *self.last_heartbeat.lock().unwrap() = beat_time;
    }

    /// Record a heartbeat from the webview (called by heartbeat_ping command)
    pub fn record_heartbeat(&self) {
        *self.last_heartbeat.lock().unwrap() = Instant::now();
    }

    /// Update panel visibility state
    pub fn set_visibility(&self, visible: bool) {
        self.is_visible.store(visible, Ordering::SeqCst);
    }

    /// Record a main thread probe response (called from main thread via run_on_main_thread)
    /// This proves the main thread is responsive
    pub fn record_main_thread_probe(&self) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.main_thread_probe_time.store(now_ms, Ordering::SeqCst);
        // Reset the reported flag when main thread responds
        self.main_thread_unresponsive_reported.store(false, Ordering::SeqCst);
    }

    /// Check if main thread is unresponsive
    /// Returns Some(stalled_ms) if main thread hasn't responded within threshold
    pub fn check_main_thread(&self) -> Option<u64> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let last_probe_ms = self.main_thread_probe_time.load(Ordering::SeqCst);
        let stalled_ms = now_ms.saturating_sub(last_probe_ms);
        
        if stalled_ms > MAIN_THREAD_PROBE_TIMEOUT.as_millis() as u64 {
            Some(stalled_ms)
        } else {
            None
        }
    }

    /// Check if the webview is responsive.
    /// Returns Some(state_change) if state changed, None otherwise.
    pub fn check(&self) -> Option<WatchdogStateChange> {
        let now = Instant::now();

        // Check for sleep/wake: if time since last check is much larger than expected,
        // the system likely slept. Reset state to avoid false positives.
        {
            let mut last_check = self.last_check.lock().unwrap();
            let elapsed_since_last_check = now.duration_since(*last_check);
            *last_check = now;

            // If elapsed is more than 2x the check interval, we likely slept
            if elapsed_since_last_check > CHECK_INTERVAL * 2 {
                // Reset state after sleep/wake
                *self.state.lock().unwrap() = WatchdogState::Healthy;
                *self.consecutive_timeouts.lock().unwrap() = 0;
                *self.unresponsive_since.lock().unwrap() = None;
                // Reset the last heartbeat to now so we don't falsely flag
                *self.last_heartbeat.lock().unwrap() = now;
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
        let last_beat = *self.last_heartbeat.lock().unwrap();
        let time_since_beat = now.duration_since(last_beat);

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
                // Transition back to Healthy on next healthy check
                *state = WatchdogState::Healthy;
            }
            // Reset timeout counter
            *self.consecutive_timeouts.lock().unwrap() = 0;
        }

        None
    }

    /// Check and return a transition event if state changed.
    /// This is the public API used by the watchdog thread.
    /// 
    /// Also checks main thread probe: if main thread is unresponsive,
    /// returns MainThreadUnresponsive (takes priority over webview checks).
    pub fn check_and_transition(&self) -> Option<WatchdogTransition> {
        // First check main thread probe (takes priority)
        if let Some(stalled_ms) = self.check_main_thread() {
            // Only report once per stall (one-shot)
            if !self.main_thread_unresponsive_reported.swap(true, Ordering::SeqCst) {
                return Some(WatchdogTransition::MainThreadUnresponsive { stalled_ms });
            }
        }

        // Then check webview heartbeat state machine
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

    /// Start the monitoring loop (called from a dedicated thread).
    /// Calls the provided callback function when state transitions occur.
    pub fn start_monitoring_loop<F>(&self, on_transition: F)
    where
        F: Fn(WatchdogTransition) + Send + 'static,
    {
        // Initialize last heartbeat to current time
        self.record_heartbeat();

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
        // Set heartbeat to be older than threshold
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

        // First timeout - should increment counter but stay healthy
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
        assert_eq!(wd.get_consecutive_timeouts(), 1);

        // Second timeout - should transition to Unresponsive
        let change = wd.check();
        assert!(change.is_some(), "Expected state change on second timeout");
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
        wd.check(); // First timeout - counter = 1
        wd.check(); // Second timeout - becomes unresponsive
        assert_eq!(wd.get_state(), WatchdogState::Unresponsive);

        // Record new heartbeat (resets staleness)
        wd.record_heartbeat();

        // Should recover
        let change = wd.check();
        assert!(change.is_some(), "Expected recovery state change");
        match change.unwrap() {
            WatchdogStateChange::Recovered { freeze_duration_ms: _ } => {}
            other => panic!("Expected Recovered, got {:?}", other),
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

        // Hide panel - next check resets counter
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
