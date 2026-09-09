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
//!
//! Sleep/wake safety:
//! - Both webview heartbeat and main thread probe use Instant (monotonic,
//!   does not advance during system sleep)
//! - Sleep/wake detection runs BEFORE native gate in check_and_transition,
//!   so the main thread probe is also reset after wake
//! - On wake detection, the main thread probe's "reported" flag is also reset
//!
//! Visibility grace period:
//! - set_visibility(true) resets last_heartbeat to now, giving the webview
//!   a grace period before the first heartbeat arrives (prevents false
//!   diag_webview_unresponsive after hide→show)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    /// App focus state (true when app is active/frontmost)
    /// FIX (fix-webview-visibility-gate): Cross-check for WebKit suspension.
    /// When the app is not active, WebKit suspends the WebContent process,
    /// causing document.visibilityState to become "hidden" and heartbeat_ping to stop.
    /// But the panel window may still be on-screen (is_visible=true).
    /// Before penalizing heartbeat misses, check if the app is focused.
    /// If not focused, WebKit suspension is expected → exempt from penalty.
    ///
    /// FIX (fix-focus-gate-edges): Defaults to FALSE (exempt-by-default).
    /// The cross-check is only meaningful after a Focused(true) event has actually
    /// been observed. If the panel is shown without activating the app (non-key
    /// window) or focus events are missed, an unknown focus state must degrade to
    /// exemption — the same failure direction as the #88 ruling — not to penalty.
    /// A default of true silently reverts to pre-#88 behavior in exactly the
    /// scenarios the cross-check was built for.
    app_focused: AtomicBool,
    /// Current state
    state: Mutex<WatchdogState>,
    /// When unresponsive state started (for freeze_duration_ms)
    unresponsive_since: Mutex<Option<Instant>>,
    /// Consecutive timeout count
    consecutive_timeouts: Mutex<u32>,
    /// Last time we checked (for detecting sleep/wake)
    last_check: Mutex<Instant>,
    /// Last main thread probe response time (monotonic Instant, NOT wall clock)
    /// FIX (fix-watchdog-false-positives): Changed from AtomicU64/SystemTime to
    /// Mutex<Instant> so sleep/wake doesn't produce false positives. SystemTime
    /// wall clock advances during sleep, making the probe appear stale after wake.
    main_thread_probe_time: Mutex<Instant>,
    /// Track if we've already reported main thread unresponsive (one-shot)
    main_thread_unresponsive_reported: AtomicBool,
}

static WATCHDOG: OnceLock<Watchdog> = OnceLock::new();

impl Watchdog {
    /// Create a new watchdog instance
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_heartbeat: Mutex::new(now),
            is_visible: AtomicBool::new(false),
            // FIX (fix-focus-gate-edges): exempt-by-default until Focused(true)
            // is actually observed (see field docs)
            app_focused: AtomicBool::new(false),
            state: Mutex::new(WatchdogState::Healthy),
            unresponsive_since: Mutex::new(None),
            consecutive_timeouts: Mutex::new(0),
            last_check: Mutex::new(now),
            main_thread_probe_time: Mutex::new(now),
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

    /// Update panel visibility state.
    ///
    /// FIX (fix-watchdog-false-positives): When transitioning to visible,
    /// reset `last_heartbeat` to now. This gives the webview a grace period
    /// before the first JS heartbeat arrives (≤15s per analytics.ts:288-297).
    /// Without this, after a long hidden period (>45s), the stale last_heartbeat
    /// triggers false diag_webview_unresponsive+recovered pairs before the first
    /// post-show heartbeat arrives — violating the 2026-09-08 visibility ruling.
    pub fn set_visibility(&self, visible: bool) {
        self.is_visible.store(visible, Ordering::SeqCst);
        if visible {
            // Reset heartbeat baseline on show — grace period for first JS heartbeat
            *self.last_heartbeat.lock().unwrap() = Instant::now();
            // Also reset consecutive timeouts to avoid false positives
            *self.consecutive_timeouts.lock().unwrap() = 0;
        }
    }

    /// Update app focus state.
    ///
    /// FIX (fix-webview-visibility-gate): When the app loses focus (user switches
    /// to another app), WebKit suspends the WebContent process. This causes
    /// `document.visibilityState` to become "hidden" in JS, stopping heartbeat_ping.
    /// However, the panel window may still be on-screen (is_visible=true).
    ///
    /// Cross-check: if the panel is visible but the app is not focused, WebKit
    /// suspension is expected behavior. We exempt this case from heartbeat penalty
    /// to avoid false diag_webview_unresponsive.
    ///
    /// FIX (fix-focus-gate-edges): Regaining focus also resets the heartbeat
    /// baseline (grace period), mirroring set_visibility(true). On refocus,
    /// WebKit must resume the suspended WebContent process and the JS context
    /// may be rebuilt from scratch (macOS App Nap suspension); the first
    /// post-resume heartbeat_ping can take up to one heartbeat interval (~15s,
    /// setInterval does not fire immediately). Without a grace period, the two
    /// consecutive-check rule (5s apart) fires a false unresponsive→recovered
    /// pair inside that resume window — the same false-positive shape the
    /// 2026-09-08 visibility ruling prohibits. A genuine freeze is unaffected:
    /// heartbeats stay absent, so unresponsive is simply delayed by one
    /// threshold period after the click.
    pub fn set_app_focused(&self, focused: bool) {
        self.app_focused.store(focused, Ordering::SeqCst);
        if focused {
            // Grace period for WebKit resume + first JS heartbeat after refocus
            *self.last_heartbeat.lock().unwrap() = Instant::now();
            *self.consecutive_timeouts.lock().unwrap() = 0;
        }
    }

    /// Query app focus state (used by diagnostics and tests).
    pub fn get_app_focused(&self) -> bool {
        self.app_focused.load(Ordering::SeqCst)
    }

    /// Record a main thread probe response (called from main thread via run_on_main_thread)
    /// This proves the main thread is responsive.
    ///
    /// FIX (fix-watchdog-false-positives): Uses monotonic Instant instead of
    /// SystemTime wall clock, so system sleep doesn't make the probe appear stale.
    pub fn record_main_thread_probe(&self) {
        *self.main_thread_probe_time.lock().unwrap() = Instant::now();
        // Reset the reported flag when main thread responds
        self.main_thread_unresponsive_reported.store(false, Ordering::SeqCst);
    }

    /// Check if main thread is unresponsive.
    /// Returns Some(stalled_ms) if main thread hasn't responded within threshold.
    ///
    /// FIX (fix-watchdog-false-positives): Uses monotonic Instant elapsed()
    /// instead of SystemTime subtraction. During system sleep, Instant does NOT
    /// advance, so a >10s sleep won't produce a false "stale probe" reading.
    pub fn check_main_thread(&self) -> Option<u64> {
        let last_probe = *self.main_thread_probe_time.lock().unwrap();
        let stalled = last_probe.elapsed();
        let stalled_ms = stalled.as_millis() as u64;

        if stalled > MAIN_THREAD_PROBE_TIMEOUT {
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
                // Also reset the main thread probe time so it doesn't appear stale
                // FIX (fix-watchdog-false-positives): Even though main_thread_probe_time
                // is now monotonic (won't be fooled by wall clock jump), we still reset
                // it after wake for safety — the probe thread may have been suspended too.
                *self.main_thread_probe_time.lock().unwrap() = now;
                self.main_thread_unresponsive_reported.store(false, Ordering::SeqCst);
                return None;
            }
        }

        // Only check heartbeat when panel is visible
        if !self.is_visible.load(Ordering::SeqCst) {
            // Reset consecutive timeouts when hidden (no false positives after show)
            *self.consecutive_timeouts.lock().unwrap() = 0;
            return None;
        }

        // FIX (fix-webview-visibility-gate): Cross-check app focus before penalizing.
        // If the app is not focused (user switched to another app), WebKit may have
        // suspended the WebContent process. This is expected behavior on macOS.
        // Exempt this case from heartbeat penalty to avoid false positives.
        if !self.app_focused.load(Ordering::SeqCst) {
            // App not focused → WebKit suspension expected → exempt from penalty
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
    /// FIX (fix-watchdog-false-positives): Sleep/wake detection (inside check())
    /// now runs conceptually before the native gate, because:
    /// 1. check() detects sleep/wake and resets both webview heartbeat AND main
    ///    thread probe time
    /// 2. check_and_transition calls check() which handles the reset
    /// 3. BUT the main thread probe check runs BEFORE check() in this method —
    ///    so on wake, we must also skip the native gate if sleep was detected.
    ///
    /// The solution: check_main_thread uses monotonic Instant, so it won't be
    /// fooled by wall clock jumps during sleep. Additionally, the sleep/wake
    /// detection in check() resets main_thread_probe_time for safety.
    pub fn check_and_transition(&self) -> Option<WatchdogTransition> {
        // Detect sleep/wake FIRST (before native gate) by checking if the
        // check interval elapsed much more than expected.
        // This ensures both the webview and native gates are protected.
        let now = Instant::now();
        let slept = {
            let mut last_check = self.last_check.lock().unwrap();
            let elapsed = now.duration_since(*last_check);
            *last_check = now;
            elapsed > CHECK_INTERVAL * 2
        };

        if slept {
            // System likely slept — reset ALL state to avoid false positives
            *self.state.lock().unwrap() = WatchdogState::Healthy;
            *self.consecutive_timeouts.lock().unwrap() = 0;
            *self.unresponsive_since.lock().unwrap() = None;
            *self.last_heartbeat.lock().unwrap() = now;
            *self.main_thread_probe_time.lock().unwrap() = now;
            self.main_thread_unresponsive_reported.store(false, Ordering::SeqCst);
            return None;
        }

        // Check main thread probe (takes priority over webview)
        if let Some(stalled_ms) = self.check_main_thread() {
            // Only report once per stall (one-shot)
            if !self.main_thread_unresponsive_reported.swap(true, Ordering::SeqCst) {
                return Some(WatchdogTransition::MainThreadUnresponsive { stalled_ms });
            }
        }

        // Then check webview heartbeat state machine
        // Note: check() will NOT re-detect sleep (already handled above),
        // so its sleep/wake branch is now a no-op safety net
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

    /// Get main thread probe stalled_ms (for testing)
    #[cfg(test)]
    pub fn get_main_thread_stalled_ms(&self) -> Option<u64> {
        self.check_main_thread()
    }

    /// Manually set main thread probe time to `age` in the past (for testing)
    #[cfg(test)]
    pub fn set_main_thread_probe_age(&self, age: Duration) {
        let now = Instant::now();
        let probe_time = now.checked_sub(age).unwrap_or(now);
        *self.main_thread_probe_time.lock().unwrap() = probe_time;
    }

    /// Manually set last_check to `age` in the past (for testing sleep/wake)
    #[cfg(test)]
    pub fn set_last_check_age(&self, age: Duration) {
        let now = Instant::now();
        let check_time = now.checked_sub(age).unwrap_or(now);
        *self.last_check.lock().unwrap() = check_time;
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
        // fix-focus-gate-edges: penalty path requires observed focus (default is exempt)
        wd.set_app_focused(true);
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
        // fix-focus-gate-edges: penalty path requires observed focus (default is exempt)
        wd.set_app_focused(true);
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
        // fix-focus-gate-edges: penalty path requires observed focus (default is exempt)
        wd.set_app_focused(true);
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
        // fix-focus-gate-edges: penalty path requires observed focus (default is exempt)
        wd.set_app_focused(true);
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
        // fix-focus-gate-edges: penalty path requires observed focus (default is exempt)
        wd.set_app_focused(true);
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
        wd.set_last_check_age(Duration::from_secs(60));

        // Set unresponsive state
        wd.set_state(WatchdogState::Unresponsive);

        // Check should detect sleep/wake and reset
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    // === Regression tests for fix-watchdog-false-positives ===

    /// FIX 1: Sleep/wake must not trigger false diag_native_main_unresponsive.
    /// Simulates system sleep by:
    /// - Setting last_check far in the past (simulates thread not running during sleep)
    /// - Setting main_thread_probe_time far in the past (simulates probe not serviced)
    /// With the old SystemTime implementation, after a >10s sleep the probe would appear
    /// stale and trigger a false MainThreadUnresponsive. With Instant, elapsed() won't
    /// advance during sleep, but we simulate the worst case (probe genuinely stale in
    /// wall clock) and verify that check_and_transition detects the sleep gap FIRST.
    #[test]
    fn sleep_wake_no_false_main_thread_unresponsive() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.record_heartbeat();
        wd.record_main_thread_probe();

        // Simulate sleep: last_check is 60s in the past
        wd.set_last_check_age(Duration::from_secs(60));
        // Probe is also 60s in the past (would trigger >10s threshold)
        wd.set_main_thread_probe_age(Duration::from_secs(60));

        // check_and_transition should detect sleep FIRST and return None,
        // NOT report MainThreadUnresponsive
        let result = wd.check_and_transition();
        assert!(
            result.is_none(),
            "Expected no transition after sleep/wake, got {:?}",
            result
        );
    }

    /// FIX 1b: After sleep/wake detection, the next normal cycle should also be clean.
    #[test]
    fn sleep_wake_then_normal_cycle_is_clean() {
        let wd = Watchdog::new();
        wd.set_visibility(true);

        // Simulate sleep
        wd.set_last_check_age(Duration::from_secs(60));
        wd.set_main_thread_probe_age(Duration::from_secs(60));

        // First cycle: sleep detected, resets everything
        assert!(wd.check_and_transition().is_none());

        // Record fresh probe and heartbeat
        wd.record_main_thread_probe();
        wd.record_heartbeat();

        // Second cycle: everything fresh, no transitions
        assert!(wd.check_and_transition().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    /// FIX 2: hide→show must not trigger false diag_webview_unresponsive.
    /// set_visibility(true) resets last_heartbeat, giving a grace period.
    #[test]
    fn show_resets_heartbeat_baseline() {
        let wd = Watchdog::new();

        // Panel visible, heartbeat healthy
        wd.set_visibility(true);
        wd.record_heartbeat();

        // Hide panel
        wd.set_visibility(false);

        // Simulate 120s passing while hidden (well over 45s threshold)
        // In real life, last_heartbeat would be 120s stale
        wd.set_last_heartbeat(Duration::from_secs(120));

        // Show panel — this must reset last_heartbeat to now
        wd.set_visibility(true);

        // Immediately check: should NOT be unresponsive
        // (set_visibility(true) just reset last_heartbeat to now)
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
        assert_eq!(wd.get_consecutive_timeouts(), 0);

        // Even after one more check cycle, still healthy
        // (because heartbeat was just reset to now, so time_since_beat ≈ 0)
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    /// FIX 2b: After show, even with stale heartbeat from before hide,
    /// the grace period prevents false positives.
    #[test]
    fn hidden_long_then_show_no_false_positive() {
        let wd = Watchdog::new();

        // Start visible with healthy heartbeat
        wd.set_visibility(true);
        wd.record_heartbeat();

        // Hide for a "long time" (simulate by aging the heartbeat)
        wd.set_visibility(false);
        wd.set_last_heartbeat(Duration::from_secs(300)); // 5 minutes stale

        // Show again — grace period kicks in
        wd.set_visibility(true);

        // Two consecutive checks (normally would trigger unresponsive)
        // But because set_visibility(true) reset the heartbeat, both should be clean
        assert!(wd.check().is_none());
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
        assert_eq!(wd.get_consecutive_timeouts(), 0);
    }

    /// Main thread probe uses monotonic Instant (not wall clock).
    /// Verify that a fresh probe is not flagged as stale.
    #[test]
    fn main_thread_probe_fresh_not_stale() {
        let wd = Watchdog::new();
        wd.record_main_thread_probe();

        // Fresh probe should not be stale
        assert!(wd.get_main_thread_stalled_ms().is_none());
    }

    /// Main thread probe genuinely stale (not sleep-related) is detected.
    #[test]
    fn main_thread_probe_genuinely_stale_detected() {
        let wd = Watchdog::new();

        // Set probe to 30s in the past (exceeds 10s threshold)
        wd.set_main_thread_probe_age(Duration::from_secs(30));

        // Should be detected as stale
        let stalled = wd.get_main_thread_stalled_ms();
        assert!(stalled.is_some());
        assert!(stalled.unwrap() >= 30_000);
    }

    /// check_and_transition reports MainThreadUnresponsive when probe is genuinely stale
    /// and no sleep was detected.
    #[test]
    fn genuine_main_thread_stall_reported() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.record_heartbeat();

        // Set probe to 30s in the past (genuine stall, not sleep)
        wd.set_main_thread_probe_age(Duration::from_secs(30));

        let result = wd.check_and_transition();
        assert!(result.is_some(), "Expected MainThreadUnresponsive transition");
        match result.unwrap() {
            WatchdogTransition::MainThreadUnresponsive { stalled_ms } => {
                assert!(stalled_ms >= 30_000, "stalled_ms should be >= 30000, got {}", stalled_ms);
            }
            other => panic!("Expected MainThreadUnresponsive, got {:?}", other),
        }
    }

    // === Regression tests for fix-webview-visibility-gate ===

    /// FIX (visibility-gate): Panel visible but app not focused → WebKit suspension expected.
    /// Should NOT penalize heartbeat misses in this case.
    #[test]
    fn unfocused_app_skips_heartbeat_check() {
        let wd = Watchdog::new();
        wd.set_visibility(true); // Panel is visible (on screen)
        wd.set_app_focused(false); // But app is not focused (user switched away)

        // Simulate stale heartbeat (would normally trigger unresponsive)
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));

        // Should NOT penalize because app is not focused
        assert!(wd.check().is_none(), "Should not penalize when app is not focused");
        assert_eq!(wd.get_consecutive_timeouts(), 0, "Timeout counter should be reset");
        assert_eq!(wd.get_state(), WatchdogState::Healthy, "Should remain healthy");

        // Multiple checks should also be clean
        assert!(wd.check().is_none());
        assert_eq!(wd.get_consecutive_timeouts(), 0);
    }

    /// FIX (visibility-gate): After app regains focus, normal checking resumes.
    /// fix-focus-gate-edges: refocus grants a heartbeat grace period (WebKit resume
    /// + JS context rebuild can take up to one heartbeat interval). The stale
    /// pre-suspension heartbeat must NOT be penalized across the refocus edge;
    /// checking resumes for heartbeats that go stale AFTER the refocus.
    #[test]
    fn refocused_app_resumes_checking() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_app_focused(false);
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));

        // While unfocused: no penalty
        assert!(wd.check().is_none());
        assert_eq!(wd.get_consecutive_timeouts(), 0);

        // App regains focus — grace period resets the stale heartbeat
        wd.set_app_focused(true);
        assert!(wd.check().is_none(), "Grace period must absorb stale heartbeat at refocus");
        assert!(wd.check().is_none(), "Second check within grace is also clean");
        assert_eq!(wd.get_consecutive_timeouts(), 0);
        assert_eq!(wd.get_state(), WatchdogState::Healthy);

        // A heartbeat that goes stale AFTER refocus is a genuine freeze and
        // must still be detected (grace must not mask real hangs)
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));
        assert!(wd.check().is_none(), "First timeout increments counter");
        assert_eq!(wd.get_consecutive_timeouts(), 1);
        assert!(wd.check().is_some(), "Second timeout triggers unresponsive");
        assert_eq!(wd.get_state(), WatchdogState::Unresponsive);
    }

    /// FIX (visibility-gate): Hidden panel + unfocused app → both gates skip check.
    #[test]
    fn hidden_and_unfocused_skips_check() {
        let wd = Watchdog::new();
        wd.set_visibility(false); // Panel hidden
        wd.set_app_focused(false); // App unfocused

        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(100));

        // Should skip (visibility gate fires first)
        assert!(wd.check().is_none());
        assert_eq!(wd.get_consecutive_timeouts(), 0);
    }

    /// FIX (visibility-gate): Real freeze scenario (focused app) still triggers unresponsive.
    /// This is the positive test: when app IS focused and heartbeat is stale,
    /// we should still detect the freeze.
    #[test]
    fn focused_app_with_stale_heartbeat_triggers_unresponsive() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_app_focused(true); // App IS focused

        // Simulate stale heartbeat (real freeze)
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(1));

        // First timeout
        assert!(wd.check().is_none());
        assert_eq!(wd.get_consecutive_timeouts(), 1);

        // Second timeout → unresponsive
        let change = wd.check();
        assert!(change.is_some(), "Should detect unresponsive when app is focused");
        assert_eq!(wd.get_state(), WatchdogState::Unresponsive);
    }

    // === Regression tests for fix-focus-gate-edges ===

    /// FIX (fix-focus-gate-edges): Unknown focus state must exempt, not penalize.
    /// Models the observed production pattern (2026-09-09 sessions): panel shown,
    /// heartbeat stops, but no Focused(true) was ever observed (non-activating
    /// show / missed focus events). With the old default (app_focused=true), the
    /// watchdog penalized these misses and emitted false diag_webview_unresponsive.
    #[test]
    fn never_focused_panel_exempt_from_penalty() {
        let wd = Watchdog::new();
        wd.set_visibility(true); // Panel visible on screen
        // NOTE: no set_app_focused call — focus state unknown (defaults to exempt)

        // Heartbeat stale far beyond threshold
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(30));

        // Repeated checks must not penalize while focus was never observed
        for _ in 0..4 {
            assert!(wd.check().is_none(), "Unknown focus state must not penalize");
        }
        assert_eq!(wd.get_consecutive_timeouts(), 0);
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }

    /// FIX (fix-focus-gate-edges): Focus loss mid-session keeps the exemption even
    /// after many checks (WebKit suspension can last minutes), and the focus-gain
    /// grace period absorbs the WebKit-resume window on the way back.
    #[test]
    fn suspend_resume_cycle_emits_no_false_positive() {
        let wd = Watchdog::new();
        wd.set_visibility(true);
        wd.set_app_focused(true);
        wd.record_heartbeat();

        // App loses focus — WebKit suspends WebContent, heartbeat stops
        wd.set_app_focused(false);
        wd.set_last_heartbeat(HEARTBEAT_TIMEOUT + Duration::from_secs(20));

        // Suspension lasts well past the threshold — exempt throughout
        for _ in 0..4 {
            assert!(wd.check().is_none(), "Suspended webview must stay exempt");
        }
        assert_eq!(wd.get_state(), WatchdogState::Healthy);

        // User refocuses the app — grace period absorbs the resume window
        // (first heartbeat_ping after resume can take up to one interval, ~15s)
        wd.set_app_focused(true);
        assert!(wd.check().is_none(), "Refocus grace must absorb stale heartbeat");
        assert!(wd.check().is_none());
        assert_eq!(wd.get_consecutive_timeouts(), 0);
        assert_eq!(wd.get_state(), WatchdogState::Healthy);

        // JS heartbeat resumes — healthy from here on
        wd.record_heartbeat();
        assert!(wd.check().is_none());
        assert_eq!(wd.get_state(), WatchdogState::Healthy);
    }
}
