//! Batch analytics queue (标准 v1.1 §7 + 架构裁定 3: blocking 线程).
//!
//! Replaces the per-event thread+Client pattern with:
//! - A bounded in-memory queue (Mutex<VecDeque>, capacity 500, drop oldest)
//! - A dedicated flush thread (20 items or 60s triggers POST {host}/batch/)
//! - Exponential backoff on failure (capped at 300s)
//! - Global singleton reqwest::blocking::Client
//! - Synchronous flush on exit (≤3s timeout)
//! - Dropped count carried in next event as `queue_dropped` property
//! - Per-event client-side RFC3339 UTC timestamp (VAL-REL-002)
//! - Batch-level `sent_at` for PostHog clock-skew correction (VAL-REL-002)

use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ── Constants ──────────────────────────────────────────────────────────

/// Maximum number of events held in memory before dropping oldest.
pub(crate) const QUEUE_CAPACITY: usize = 500;

/// Number of queued events that triggers an immediate flush.
const FLUSH_THRESHOLD: usize = 20;

/// Seconds between flushes when threshold is not reached.
const FLUSH_INTERVAL_SECS: u64 = 60;

/// Initial backoff delay after a failed flush (seconds).
const INITIAL_BACKOFF_SECS: u64 = 1;

/// Maximum backoff delay to prevent storm (seconds).
const MAX_BACKOFF_SECS: u64 = 300;

/// How often the flush thread wakes to check conditions (seconds).
const TICK_SECS: u64 = 1;

/// HTTP request timeout for batch sends.
const HTTP_TIMEOUT_SECS: u64 = 10;

// ── Types ──────────────────────────────────────────────────────────────

/// A single event waiting in the queue.
#[derive(Debug, Clone)]
pub(crate) struct QueueEntry {
    pub event: String,
    pub distinct_id: String,
    pub properties: Value,
    /// RFC3339 UTC timestamp captured at enqueue time (VAL-REL-002).
    pub timestamp: String,
}

/// Shared mutable state protected by the mutex.
struct QueueState {
    entries: VecDeque<QueueEntry>,
    /// Total events dropped since queue creation (monotonic).
    dropped_count: u64,
    /// Dropped events not yet reported via `queue_dropped` property.
    /// Consumed (reset to 0) when the next event picks it up.
    pending_dropped: u64,
    /// Signal for the flush thread to shut down.
    shutdown: bool,
}

/// Handle to the shared queue state.
struct Inner {
    state: Mutex<QueueState>,
    condvar: Condvar,
}

// ── Globals ────────────────────────────────────────────────────────────

static BATCH_QUEUE: OnceLock<Arc<Inner>> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
static BATCH_URL: OnceLock<String> = OnceLock::new();
static API_KEY: OnceLock<String> = OnceLock::new();

// ── Timestamp helpers (RFC3339 UTC, no chrono) ─────────────────────────

/// Generate an RFC3339 UTC timestamp for the current moment.
/// Uses only `std::time` — no chrono dependency.
/// Format: `2026-09-08T12:34:56.789Z`
pub(crate) fn now_rfc3339() -> String {
    system_time_to_rfc3339(SystemTime::now())
}

/// Convert a `SystemTime` to RFC3339 UTC string.
/// Manual formatting to avoid pulling in chrono.
pub(crate) fn system_time_to_rfc3339(time: SystemTime) -> String {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    // Break down into date/time components
    let (year, month, day, hour, min, sec) = unix_secs_to_datetime(secs);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, min, sec, millis
    )
}

/// Convert Unix timestamp (seconds since 1970-01-01 00:00:00 UTC) to
/// (year, month, day, hour, minute, second) tuple.
/// Pure arithmetic, no lookup tables, no allocations.
fn unix_secs_to_datetime(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = secs % 60;
    let mins_total = secs / 60;
    let min = mins_total % 60;
    let hours_total = mins_total / 60;
    let hour = hours_total % 24;
    let days = hours_total / 24;

    // Days since 1970-01-01
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month index [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // day [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // month [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    (y, m, d, hour, min, sec)
}

// ── Public API ─────────────────────────────────────────────────────────

/// Initialize the batch queue, HTTP client, and spawn the flush thread.
///
/// Called once during app setup. Subsequent calls are no-ops.
pub(crate) fn init(api_key: &str, host: &str) {
    let inner = Arc::new(Inner {
        state: Mutex::new(QueueState {
            entries: VecDeque::with_capacity(QUEUE_CAPACITY),
            dropped_count: 0,
            pending_dropped: 0,
            shutdown: false,
        }),
        condvar: Condvar::new(),
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .expect("Failed to create HTTP client for analytics");

    let _ = HTTP_CLIENT.set(client);
    let _ = BATCH_URL.set(format!("{}/batch/", host.trim_end_matches('/')));
    let _ = API_KEY.set(api_key.to_string());

    // Spawn flush thread before setting BATCH_QUEUE so that the thread
    // can start working immediately once events are enqueued.
    let flush_inner = inner.clone();
    std::thread::Builder::new()
        .name("analytics-flush".into())
        .spawn(move || flush_loop(flush_inner))
        .expect("Failed to spawn analytics flush thread");

    let _ = BATCH_QUEUE.set(inner);
}

/// Enqueue an event for batch sending.
///
/// If the queue is at capacity, the oldest entry is dropped and the
/// dropped counter is incremented. The dropped count is carried as
/// `queue_dropped` in the next event enqueued after overflow.
///
/// This function is safe to call before `init()` — events are silently
/// dropped in that case (graceful degradation).
pub(crate) fn enqueue(event: &str, distinct_id: &str, mut properties: Value) {
    let Some(inner) = BATCH_QUEUE.get() else {
        return;
    };
    enqueue_inner(inner, event, distinct_id, &mut properties);
}

/// Synchronously flush remaining events with a timeout.
///
/// Used during `RunEvent::Exit` to ensure the last events (including
/// `rust_exit`) reach PostHog before the process terminates.
///
/// Returns `true` if all queued events were successfully sent.
pub(crate) fn flush_sync(timeout: Duration) -> bool {
    let Some(inner) = BATCH_QUEUE.get() else {
        return true;
    };
    let Some(client) = HTTP_CLIENT.get() else {
        return true; // No client = no key, nothing to send
    };
    let Some(url) = BATCH_URL.get() else {
        return true;
    };
    let Some(api_key) = API_KEY.get() else {
        return true;
    };

    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }

        // Drain current entries
        let entries = {
            let mut state = inner.state.lock().unwrap();
            if state.entries.is_empty() {
                return true;
            }
            state.entries.drain(..).collect::<Vec<_>>()
        };

        if entries.is_empty() {
            return true;
        }

        let body = build_batch_body(api_key, &entries);
        match client
            .post(url.as_str())
            .json(&body)
            .timeout(remaining)
            .send()
        {
            Ok(resp) if resp.status().is_success() => {
                // Success — loop to check if more entries accumulated
                continue;
            }
            _ => {
                // Failure or timeout — put entries back
                let mut state = inner.state.lock().unwrap();
                for entry in entries.into_iter().rev() {
                    state.entries.push_front(entry);
                }
                return false;
            }
        }
    }
}

// ── Internal helpers ───────────────────────────────────────────────────

/// Core enqueue logic operating on an `Inner` reference (testable).
fn enqueue_inner(inner: &Inner, event: &str, distinct_id: &str, properties: &mut Value) {
    // Attach pending dropped count to this event (if any)
    let pending = {
        let mut state = inner.state.lock().unwrap();
        let p = state.pending_dropped;
        state.pending_dropped = 0;
        p
    };
    if pending > 0 {
        if let Some(obj) = properties.as_object_mut() {
            obj.insert(
                "queue_dropped".to_string(),
                Value::Number(pending.into()),
            );
        }
    }

    // Capture timestamp at enqueue moment (VAL-REL-002).
    // This timestamp is immutable — it survives serialization, batching,
    // and persistence without changing.
    let ts = now_rfc3339();

    // Also inject timestamp into properties so it's queryable in PostHog
    if let Some(obj) = properties.as_object_mut() {
        obj.insert("timestamp".to_string(), Value::String(ts.clone()));
    }

    let entry = QueueEntry {
        event: event.to_string(),
        distinct_id: distinct_id.to_string(),
        properties: properties.clone(),
        timestamp: ts,
    };

    let mut state = inner.state.lock().unwrap();
    if state.entries.len() >= QUEUE_CAPACITY {
        state.entries.pop_front(); // Drop oldest
        state.dropped_count += 1;
        state.pending_dropped += 1;
    }
    state.entries.push_back(entry);

    // Notify flush thread
    inner.condvar.notify_one();
}

/// Build the JSON body for a /batch/ request.
///
/// Each event entry carries its own `timestamp` (RFC3339 UTC, captured at
/// enqueue time) inside `properties` so PostHog can distinguish events within
/// the same batch (VAL-REL-002). The top-level `sent_at` records the moment
/// the HTTP request is assembled, enabling clock-skew correction.
fn build_batch_body(api_key: &str, entries: &[QueueEntry]) -> Value {
    let batch: Vec<Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "event": e.event,
                "distinct_id": e.distinct_id,
                "properties": e.properties,
                "timestamp": e.timestamp,
            })
        })
        .collect();

    serde_json::json!({
        "api_key": api_key,
        "batch": batch,
        "sent_at": now_rfc3339(),
    })
}

/// Compute the next backoff duration (exponential with cap).
fn next_backoff(current_secs: u64) -> u64 {
    std::cmp::min(current_secs.saturating_mul(2), MAX_BACKOFF_SECS)
}

/// Compute backoff with jitter to prevent thundering herd.
/// Returns a duration in the range [delay * 0.5, delay * 1.0]
fn next_backoff_with_jitter(delay_secs: u64) -> u64 {
    let mut random_bytes = [0u8; 8];
    getrandom::getrandom(&mut random_bytes).unwrap_or_else(|_| {
        // Fallback: use system time if getrandom fails
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .subsec_nanos() as u64;
        random_bytes = nanos.to_le_bytes();
    });
    
    // Convert to u64 and get a value in [0.0, 1.0)
    let random_u64 = u64::from_le_bytes(random_bytes);
    let jitter_factor = (random_u64 % 5000) as f64 / 10000.0; // [0.0, 0.5)
    
    let jittered = delay_secs as f64 * (0.5 + jitter_factor);
    jittered.round() as u64
}

/// Check whether flush conditions are met.
fn should_flush(entry_count: usize, elapsed_since_last_flush: Duration) -> bool {
    entry_count >= FLUSH_THRESHOLD
        || (entry_count > 0 && elapsed_since_last_flush >= Duration::from_secs(FLUSH_INTERVAL_SECS))
}

/// The flush thread main loop.
fn flush_loop(inner: Arc<Inner>) {
    let mut backoff_secs: u64 = INITIAL_BACKOFF_SECS;
    let mut last_flush = Instant::now();

    loop {
        // Wait for notification or tick
        {
            let state = inner.state.lock().unwrap();
            if state.shutdown {
                return;
            }
            let (_state, _) = inner
                .condvar
                .wait_timeout(state, Duration::from_secs(TICK_SECS))
                .unwrap();
        }

        // Check shutdown
        {
            let state = inner.state.lock().unwrap();
            if state.shutdown {
                return;
            }
        }

        // Check flush conditions
        let do_flush = {
            let state = inner.state.lock().unwrap();
            should_flush(state.entries.len(), last_flush.elapsed())
        };

        if !do_flush {
            continue;
        }

        // Drain entries
        let entries = {
            let mut state = inner.state.lock().unwrap();
            state.entries.drain(..).collect::<Vec<_>>()
        };

        if entries.is_empty() {
            continue;
        }

        // Send batch
        let client = match HTTP_CLIENT.get() {
            Some(c) => c,
            None => continue,
        };
        let url = match BATCH_URL.get() {
            Some(u) => u.clone(),
            None => continue,
        };
        let api_key = match API_KEY.get() {
            Some(k) => k.clone(),
            None => continue,
        };

        let body = build_batch_body(&api_key, &entries);
        match client.post(&url).json(&body).send() {
            Ok(resp) if resp.status().is_success() => {
                backoff_secs = INITIAL_BACKOFF_SECS;
                last_flush = Instant::now();
            }
            _ => {
                // Put entries back at front
                let mut state = inner.state.lock().unwrap();
                for entry in entries.into_iter().rev() {
                    state.entries.push_front(entry);
                }
                // Enforce capacity after re-insertion
                while state.entries.len() > QUEUE_CAPACITY {
                    state.entries.pop_front();
                    state.dropped_count += 1;
                    state.pending_dropped += 1;
                }
                drop(state);

                // Exponential backoff with jitter
                std::thread::sleep(Duration::from_secs(next_backoff_with_jitter(backoff_secs)));
                backoff_secs = next_backoff(backoff_secs);
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a fresh Inner for isolated testing (no global state).
    fn make_inner() -> Inner {
        Inner {
            state: Mutex::new(QueueState {
                entries: VecDeque::with_capacity(QUEUE_CAPACITY),
                dropped_count: 0,
                pending_dropped: 0,
                shutdown: false,
            }),
            condvar: Condvar::new(),
        }
    }

    fn make_entry(event: &str) -> QueueEntry {
        QueueEntry {
            event: event.to_string(),
            distinct_id: "test-user".to_string(),
            properties: serde_json::json!({}),
            timestamp: now_rfc3339(),
        }
    }

    /// Helper: directly push an entry into the queue (bypassing enqueue_inner
    /// to avoid the pending_dropped side-effect for capacity tests).
    fn push_entry(inner: &Inner, entry: QueueEntry) {
        let mut state = inner.state.lock().unwrap();
        if state.entries.len() >= QUEUE_CAPACITY {
            state.entries.pop_front();
            state.dropped_count += 1;
            state.pending_dropped += 1;
        }
        state.entries.push_back(entry);
    }

    // ── Queue capacity tests (VAL-REL-005) ──

    #[test]
    fn queue_accepts_up_to_capacity() {
        let inner = make_inner();
        for i in 0..QUEUE_CAPACITY {
            push_entry(&inner, make_entry(&format!("event_{}", i)));
        }
        let state = inner.state.lock().unwrap();
        assert_eq!(state.entries.len(), QUEUE_CAPACITY);
        assert_eq!(state.dropped_count, 0);
    }

    #[test]
    fn queue_capacity_500_drop_oldest() {
        let inner = make_inner();
        // Fill to capacity
        for i in 0..QUEUE_CAPACITY {
            push_entry(&inner, make_entry(&format!("event_{}", i)));
        }
        // One more — should drop event_0
        push_entry(&inner, make_entry("event_overflow"));

        let state = inner.state.lock().unwrap();
        assert_eq!(state.entries.len(), QUEUE_CAPACITY);
        assert_eq!(state.entries[0].event, "event_1", "oldest (event_0) should be dropped");
        assert_eq!(
            state.entries[QUEUE_CAPACITY - 1].event,
            "event_overflow",
            "newest should be at back"
        );
        assert_eq!(state.dropped_count, 1);
    }

    #[test]
    fn queue_dropped_count_accumulates() {
        let inner = make_inner();
        // Fill + overflow by 10
        for i in 0..QUEUE_CAPACITY + 10 {
            push_entry(&inner, make_entry(&format!("event_{}", i)));
        }
        let state = inner.state.lock().unwrap();
        assert_eq!(state.entries.len(), QUEUE_CAPACITY);
        assert_eq!(state.dropped_count, 10);
        assert_eq!(state.pending_dropped, 10);
        // First 10 events were dropped
        assert_eq!(state.entries[0].event, "event_10");
    }

    #[test]
    fn queue_does_not_crash_on_massive_overflow() {
        let inner = make_inner();
        // Overflow by 1000 — process must not crash
        for i in 0..QUEUE_CAPACITY + 1000 {
            push_entry(&inner, make_entry(&format!("event_{}", i)));
        }
        let state = inner.state.lock().unwrap();
        assert_eq!(state.entries.len(), QUEUE_CAPACITY);
        assert_eq!(state.dropped_count, 1000);
    }

    // ── Dropped count carry-over tests ──

    #[test]
    fn pending_dropped_attached_to_next_event() {
        let inner = make_inner();
        // Overflow by 5
        for i in 0..QUEUE_CAPACITY + 5 {
            push_entry(&inner, make_entry(&format!("event_{}", i)));
        }
        {
            let state = inner.state.lock().unwrap();
            assert_eq!(state.pending_dropped, 5);
        }

        // Now enqueue via enqueue_inner — should pick up pending_dropped
        let mut props = serde_json::json!({"key": "value"});
        enqueue_inner(&inner, "recovery_event", "user1", &mut props);

        // The event should now carry queue_dropped=5
        assert_eq!(props["queue_dropped"], 5);

        // Pending should be 1 because the new entry caused another drop
        let state = inner.state.lock().unwrap();
        assert_eq!(state.pending_dropped, 1);
    }

    #[test]
    fn pending_dropped_reset_after_consumed() {
        let inner = make_inner();
        // Overflow by 3
        for i in 0..QUEUE_CAPACITY + 3 {
            push_entry(&inner, make_entry(&format!("event_{}", i)));
        }

        // First enqueue picks up the count
        let mut props1 = serde_json::json!({});
        enqueue_inner(&inner, "first", "u", &mut props1);
        assert_eq!(props1["queue_dropped"], 3);

        // Second enqueue picks up the new pending (from the drop caused by first)
        let mut props2 = serde_json::json!({});
        enqueue_inner(&inner, "second", "u", &mut props2);
        assert_eq!(props2["queue_dropped"], 1, "should carry the drop from the previous enqueue");
        
        // Third enqueue should have no pending (if we don't overflow again)
        // But since queue is still full, it will drop one, so pending=1
        let mut props3 = serde_json::json!({});
        enqueue_inner(&inner, "third", "u", &mut props3);
        assert_eq!(props3["queue_dropped"], 1);
    }

    #[test]
    fn no_queue_dropped_when_no_overflow() {
        let inner = make_inner();
        // Normal enqueue without overflow
        let mut props = serde_json::json!({"key": "val"});
        enqueue_inner(&inner, "normal", "user", &mut props);
        assert!(
            props.get("queue_dropped").is_none(),
            "no queue_dropped when no overflow occurred"
        );
    }

    // ── Batch body structure tests (VAL-REL-001) ──

    #[test]
    fn batch_body_has_correct_structure() {
        let entries = vec![
            QueueEntry {
                event: "rust_launch".to_string(),
                distinct_id: "desktop-abc".to_string(),
                properties: serde_json::json!({"session_id": "s1", "source": "rust_native"}),
                timestamp: "2026-09-08T12:00:00.000Z".to_string(),
            },
            QueueEntry {
                event: "rust_heartbeat".to_string(),
                distinct_id: "desktop-abc".to_string(),
                properties: serde_json::json!({"session_id": "s1", "seq": 1}),
                timestamp: "2026-09-08T12:00:05.000Z".to_string(),
            },
        ];

        let body = build_batch_body("test_key_123", &entries);

        // Top-level fields
        assert_eq!(body["api_key"], "test_key_123");
        let batch = body["batch"].as_array().expect("batch must be an array");
        assert_eq!(batch.len(), 2);

        // First entry
        assert_eq!(batch[0]["event"], "rust_launch");
        assert_eq!(batch[0]["distinct_id"], "desktop-abc");
        assert_eq!(batch[0]["properties"]["session_id"], "s1");
        assert_eq!(batch[0]["properties"]["source"], "rust_native");

        // Second entry
        assert_eq!(batch[1]["event"], "rust_heartbeat");
        assert_eq!(batch[1]["distinct_id"], "desktop-abc");
        assert_eq!(batch[1]["properties"]["seq"], 1);
    }

    #[test]
    fn batch_body_empty_entries() {
        let body = build_batch_body("key", &[]);
        assert_eq!(body["api_key"], "key");
        assert_eq!(body["batch"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn batch_body_preserves_properties() {
        let entries = vec![QueueEntry {
            event: "test".to_string(),
            distinct_id: "user".to_string(),
            properties: serde_json::json!({
                "app_version": "0.2.4",
                "environment": "production",
                "platform": "macos",
                "arch": "aarch64",
                "queue_dropped": 42,
            }),
            timestamp: "2026-09-08T12:00:00.000Z".to_string(),
        }];

        let body = build_batch_body("k", &entries);
        let props = &body["batch"][0]["properties"];
        assert_eq!(props["app_version"], "0.2.4");
        assert_eq!(props["environment"], "production");
        assert_eq!(props["queue_dropped"], 42);
    }

    // ── Flush condition tests ──

    #[test]
    fn flush_triggered_at_20_items() {
        assert!(should_flush(20, Duration::from_secs(0)));
        assert!(should_flush(100, Duration::from_secs(0)));
    }

    #[test]
    fn flush_not_triggered_below_threshold_before_interval() {
        assert!(!should_flush(19, Duration::from_secs(30)));
        assert!(!should_flush(1, Duration::from_secs(59)));
        assert!(!should_flush(0, Duration::from_secs(120)));
    }

    #[test]
    fn flush_triggered_by_time_interval() {
        assert!(should_flush(1, Duration::from_secs(60)));
        assert!(should_flush(5, Duration::from_secs(61)));
        assert!(should_flush(1, Duration::from_secs(120)));
    }

    // ── Exponential backoff tests ──

    #[test]
    fn backoff_starts_at_one_second() {
        assert_eq!(INITIAL_BACKOFF_SECS, 1);
    }

    #[test]
    fn backoff_doubles_each_time() {
        let mut b = INITIAL_BACKOFF_SECS;
        let mut sequence = vec![];
        for _ in 0..10 {
            sequence.push(b);
            b = next_backoff(b);
        }
        assert_eq!(sequence, vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 300]);
    }

    #[test]
    fn backoff_capped_at_max() {
        assert_eq!(next_backoff(MAX_BACKOFF_SECS), MAX_BACKOFF_SECS);
        assert_eq!(next_backoff(MAX_BACKOFF_SECS + 100), MAX_BACKOFF_SECS);
    }

    #[test]
    fn backoff_does_not_overflow() {
        // Even with very large input, should cap gracefully
        assert_eq!(next_backoff(u64::MAX), MAX_BACKOFF_SECS);
    }

    // ── URL construction ──

    #[test]
    fn batch_url_format() {
        let host = "https://us.i.posthog.com";
        let url = format!("{}/batch/", host.trim_end_matches('/'));
        assert_eq!(url, "https://us.i.posthog.com/batch/");
    }

    #[test]
    fn batch_url_trailing_slash_normalized() {
        let host = "https://us.i.posthog.com/";
        let url = format!("{}/batch/", host.trim_end_matches('/'));
        assert_eq!(url, "https://us.i.posthog.com/batch/");
    }

    // ── Timestamp tests (VAL-REL-002) ──

    #[test]
    fn rfc3339_format_valid() {
        let ts = now_rfc3339();
        // Should match pattern: YYYY-MM-DDTHH:MM:SS.mmmZ
        assert!(ts.ends_with('Z'), "timestamp must end with Z: {}", ts);
        assert_eq!(ts.len(), 24, "timestamp length should be 24 chars: {}", ts);
        assert!(ts.contains('T'), "timestamp must contain T separator");
    }

    #[test]
    fn rfc3339_unix_epoch() {
        // Test known timestamp: 1970-01-01 00:00:00 UTC
        let epoch = UNIX_EPOCH;
        let ts = system_time_to_rfc3339(epoch);
        assert_eq!(ts, "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn rfc3339_known_date() {
        // Test 2024-01-15 09:30:45.123 UTC
        // Unix timestamp for 2024-01-15 09:30:45 UTC = 1705311045
        let secs = 1705311045;
        let millis = 123;
        let duration = Duration::new(secs, millis * 1_000_000);
        let time = UNIX_EPOCH + duration;
        let ts = system_time_to_rfc3339(time);
        assert_eq!(ts, "2024-01-15T09:30:45.123Z");
    }

    #[test]
    fn batch_body_includes_sent_at() {
        let entries = vec![QueueEntry {
            event: "test".to_string(),
            distinct_id: "user".to_string(),
            properties: serde_json::json!({}),
            timestamp: "2026-09-08T12:00:00.000Z".to_string(),
        }];
        let body = build_batch_body("key", &entries);
        
        // sent_at must be present at top level
        assert!(body.get("sent_at").is_some(), "batch body must include sent_at");
        let sent_at = body["sent_at"].as_str().unwrap();
        assert!(sent_at.ends_with('Z'), "sent_at must be RFC3339 UTC");
    }

    #[test]
    fn batch_body_includes_event_timestamp() {
        let entries = vec![QueueEntry {
            event: "rust_launch".to_string(),
            distinct_id: "desktop-abc".to_string(),
            properties: serde_json::json!({}),
            timestamp: "2026-09-08T12:00:00.000Z".to_string(),
        }];
        let body = build_batch_body("key", &entries);
        
        // Each event must have its timestamp
        let batch = body["batch"].as_array().unwrap();
        assert_eq!(batch[0]["timestamp"], "2026-09-08T12:00:00.000Z");
    }

    #[test]
    fn enqueue_captures_timestamp() {
        let inner = make_inner();
        let mut props = serde_json::json!({});
        let before = now_rfc3339();
        std::thread::sleep(Duration::from_millis(10));
        enqueue_inner(&inner, "test_event", "user", &mut props);
        std::thread::sleep(Duration::from_millis(10));
        let after = now_rfc3339();

        let state = inner.state.lock().unwrap();
        assert_eq!(state.entries.len(), 1);
        let ts = &state.entries[0].timestamp;
        
        // Timestamp should be between before and after
        assert!(ts >= &before && ts <= &after, "timestamp should be captured at enqueue time");
    }

    #[test]
    fn timestamps_preserve_order() {
        let inner = make_inner();
        
        let mut props1 = serde_json::json!({});
        enqueue_inner(&inner, "event1", "user", &mut props1);
        std::thread::sleep(Duration::from_millis(50));
        
        let mut props2 = serde_json::json!({});
        enqueue_inner(&inner, "event2", "user", &mut props2);
        
        let state = inner.state.lock().unwrap();
        assert_eq!(state.entries.len(), 2);
        
        // Second event's timestamp should be >= first event's timestamp
        let ts1 = &state.entries[0].timestamp;
        let ts2 = &state.entries[1].timestamp;
        assert!(ts2 >= ts1, "timestamps should preserve chronological order");
    }

    // ── Jitter tests (标准 §7, TDD RED phase) ──

    #[test]
    fn jitter_within_bounds() {
        // For a delay of 10s, jittered value should be in [5, 10]
        let delay = 10u64;
        for _ in 0..100 {
            let jittered = next_backoff_with_jitter(delay);
            assert!(
                jittered >= delay / 2 && jittered <= delay,
                "jittered value {} should be in [{}, {}]",
                jittered,
                delay / 2,
                delay
            );
        }
    }

    #[test]
    fn jitter_with_zero_delay() {
        // Zero delay should return zero
        let jittered = next_backoff_with_jitter(0);
        assert_eq!(jittered, 0);
    }

    #[test]
    fn jitter_distribution_varies() {
        // Run 1000 iterations and verify we get at least 2 different values
        let delay = 10u64;
        let mut results = std::collections::HashSet::new();
        for _ in 0..1000 {
            results.insert(next_backoff_with_jitter(delay));
        }
        assert!(
            results.len() > 1,
            "jitter should produce varied results, got only {} unique values",
            results.len()
        );
    }
}
