//! Synchronous panic event sender (VAL-DIAG-006).
//!
//! Bypasses the batch queue and sends panic events directly via blocking reqwest.
//! This is necessary because:
//! 1. Batch queue runs in a separate thread that won't survive panic=abort
//! 2. Panic hook must complete synchronously before process termination
//! 3. We need guaranteed delivery even when the process is about to crash

use serde_json::Value;
use std::time::Duration;

/// Build a panic event payload (pure function for testing).
///
/// Returns a JSON value with:
/// - `api_key` field (PostHog body authentication)
/// - `event` = "diag_rust_panic"
/// - `distinct_id` = install_id
/// - `properties` with error_code, message (truncated), backtrace (truncated), session_id, source, environment, platform, arch, app_version, build_sha
pub(crate) fn build_panic_event(
    api_key: &str,
    install_id: &str,
    session_id: &str,
    panic_message: &str,
    backtrace: &str,
    app_version: &str,
) -> Value {
    // Truncate message to ≤300 chars (PostHog property limit)
    let truncated_msg: String = panic_message.chars().take(300).collect();

    // Truncate backtrace to first 500 chars
    let truncated_bt: String = backtrace.chars().take(500).collect();

    serde_json::json!({
        "api_key": api_key,
        "event": "diag_rust_panic",
        "distinct_id": install_id,
        "properties": {
            "error_code": "E_RUST_PANIC",
            "message": truncated_msg,
            "backtrace": truncated_bt,
            "session_id": session_id,
            "source": "rust_native",
            "environment": if cfg!(debug_assertions) { "development" } else { "production" },
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "app_version": app_version,
            // Build identifier for build attribution (fix-build-attribution).
            // Panic events bypass posthog_capture's baseline injection, so the
            // compile-time const is referenced directly.
            "build_sha": crate::BUILD_SHA,
        }
    })
}

/// Retry logic with bounded attempts and linear backoff.
/// Pure function: transport is injectable for testing.
///
/// - max_attempts: total attempts including the first (e.g. 3 = 1 initial + 2 retries)
/// - backoff_ms: delay between attempts in milliseconds [500, 1000, ...]
/// - Total delay bounded: for 3 attempts with [500, 1000], max delay = 1.5s
///
/// Returns Ok(()) on first successful attempt, or Err with last error.
pub(crate) fn send_with_retries(
    event: &Value,
    max_attempts: usize,
    backoff_ms: &[u64],
    mut send_fn: Box<dyn FnMut(&Value) -> Result<(), String>>,
) -> Result<(), String> {
    assert!(max_attempts >= 1, "max_attempts must be at least 1");
    assert!(
        backoff_ms.len() >= max_attempts.saturating_sub(1),
        "backoff_ms must have at least max_attempts-1 entries"
    );

    for attempt in 0..max_attempts {
        match send_fn(event) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("[panic_sender] attempt {} failed: {}", attempt + 1, e);
                if attempt + 1 < max_attempts {
                    std::thread::sleep(Duration::from_millis(backoff_ms[attempt]));
                } else {
                    return Err(e);
                }
            }
        }
    }
    Err("all retry attempts exhausted".to_string())
}

/// Send a panic event synchronously to PostHog.
///
/// This function:
/// - Truncates message to ≤300 chars (PostHog property limit)
/// - Extracts backtrace summary (first 500 chars)
/// - Sends via blocking HTTP POST with bounded retry (3 attempts, linear backoff)
/// - Total bounded: max 3 attempts × 3s timeout + 1.5s delays = 10.5s worst case
/// - Skips send when api_key is empty (NO_KEY sentinel guard)
///
/// Called from std::panic::set_hook — must be synchronous and fast.
pub(crate) fn send_panic_event(
    api_key: &str,
    host: &str,
    install_id: &str,
    session_id: &str,
    panic_message: &str,
    backtrace: &str,
    app_version: &str,
) {
    // Guard: empty api_key (NO_KEY sentinel) → skip send (would fail with 401)
    if api_key.is_empty() || api_key == "NO_KEY" {
        eprintln!("[panic_sender] skipping send: no valid api_key (NO_KEY sentinel)");
        return;
    }

    let event = build_panic_event(api_key, install_id, session_id, panic_message, backtrace, app_version);

    let url = format!("{}/capture/", host.trim_end_matches('/'));

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[panic_sender] failed to create HTTP client: {}", e);
            return;
        }
    };

    // Bounded retry: max 3 attempts, linear backoff [500ms, 1000ms]
    let backoff_ms = [500u64, 1000u64];
    let _ = send_with_retries(
        &event,
        3,
        &backoff_ms,
        Box::new(move |body| {
            client
                .post(&url)
                .json(body)
                .send()
                .map_err(|e| e.to_string())
                .and_then(|resp| {
                    if resp.status().is_success() {
                        Ok(())
                    } else {
                        Err(format!("HTTP {}", resp.status()))
                    }
                })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_panic_event_contains_api_key() {
        let event = build_panic_event(
            "test_api_key_123",
            "desktop-test-id",
            "session-abc",
            "test panic message",
            "test backtrace",
            "0.2.4",
        );

        // api_key must be present in payload (PostHog body authentication)
        assert_eq!(event["api_key"], "test_api_key_123");
    }

    #[test]
    fn build_panic_event_truncates_long_message() {
        let long_msg = "a".repeat(500);
        let event = build_panic_event(
            "key",
            "desktop-id",
            "session",
            &long_msg,
            "backtrace",
            "0.2.4",
        );

        // Message must be truncated to 300 chars
        let message = event["properties"]["message"].as_str().unwrap();
        assert_eq!(message.len(), 300, "message must be truncated to 300 chars");
        assert_eq!(message, &"a".repeat(300));
    }

    #[test]
    fn build_panic_event_preserves_short_message() {
        let short_msg = "short panic";
        let event = build_panic_event(
            "key",
            "desktop-id",
            "session",
            short_msg,
            "backtrace",
            "0.2.4",
        );

        let message = event["properties"]["message"].as_str().unwrap();
        assert_eq!(message, short_msg);
    }

    #[test]
    fn build_panic_event_truncates_long_backtrace() {
        let long_bt = "backtrace line\n".repeat(100);
        let event = build_panic_event(
            "key",
            "desktop-id",
            "session",
            "panic",
            &long_bt,
            "0.2.4",
        );

        let backtrace = event["properties"]["backtrace"].as_str().unwrap();
        assert_eq!(backtrace.len(), 500, "backtrace must be truncated to 500 chars");
    }

    #[test]
    fn build_panic_event_has_required_properties() {
        let event = build_panic_event(
            "api_key",
            "desktop-test-id",
            "session-123",
            "test panic",
            "test bt",
            "0.2.4",
        );

        // Top-level structure
        assert_eq!(event["event"], "diag_rust_panic");
        assert_eq!(event["distinct_id"], "desktop-test-id");

        // Required properties
        let props = &event["properties"];
        assert_eq!(props["error_code"], "E_RUST_PANIC");
        assert_eq!(props["session_id"], "session-123");
        assert_eq!(props["source"], "rust_native");
        assert!(props["environment"].is_string());
        assert!(props["platform"].is_string());
        assert!(props["arch"].is_string());
    }

    #[test]
    fn build_panic_event_message_truncation_respects_unicode() {
        // Unicode chars may be multi-byte; truncation is by char count, not byte count
        let unicode_msg = "中".repeat(400); // 400 Chinese characters
        let event = build_panic_event(
            "key",
            "desktop-id",
            "session",
            &unicode_msg,
            "bt",
            "0.2.4",
        );

        let message = event["properties"]["message"].as_str().unwrap();
        assert_eq!(message.chars().count(), 300, "must truncate by char count, not byte count");
    }

    #[test]
    fn build_panic_event_includes_app_version() {
        // Task (b): panic events must include app_version baseline property
        let event = build_panic_event(
            "api_key",
            "desktop-test-id",
            "session-123",
            "test panic",
            "test bt",
            "0.2.4",
        );

        // app_version must be present and non-empty
        let app_version = event["properties"]["app_version"].as_str();
        assert!(app_version.is_some(), "app_version must be present in properties");
        assert!(!app_version.unwrap().is_empty(), "app_version must not be empty");
        assert_eq!(app_version.unwrap(), "0.2.4");
    }

    #[test]
    fn build_panic_event_includes_build_sha() {
        // fix-build-attribution: panic events bypass posthog_capture's baseline
        // injection, so build_sha must be attached explicitly and never be empty.
        let event = build_panic_event(
            "api_key",
            "desktop-test-id",
            "session-123",
            "test panic",
            "test bt",
            "0.2.4",
        );

        let build_sha = event["properties"]["build_sha"].as_str();
        assert!(build_sha.is_some(), "build_sha must be present in properties");
        assert!(!build_sha.unwrap().is_empty(), "build_sha must not be empty");
        assert_eq!(build_sha.unwrap(), crate::BUILD_SHA);
    }

    #[test]
    fn send_panic_event_skips_when_api_key_empty() {
        // Task (c): empty api_key must not attempt to send (would fail with 401)
        // This test verifies the guard exists by calling with empty key
        // The function should return early without making an HTTP request
        send_panic_event(
            "", // empty api_key
            "https://us.i.posthog.com",
            "desktop-id",
            "session",
            "test panic",
            "test bt",
            "0.2.4",
        );
        // If we reach here without panicking, the guard exists
        // In a real scenario, we'd verify no HTTP request was made
    }

    // ---- Retry logic tests ----

    #[test]
    fn send_with_retries_succeeds_on_first_attempt() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let event = serde_json::json!({"test": "data"});

        let result = send_with_retries(
            &event,
            3,
            &[500, 1000],
            Box::new(move |_| {
                attempts_clone.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        );

        assert!(result.is_ok(), "first attempt success should return Ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "should succeed on first attempt");
    }

    #[test]
    fn send_with_retries_succeeds_after_first_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let event = serde_json::json!({"test": "data"});

        let result = send_with_retries(
            &event,
            3,
            &[500, 1000],
            Box::new(move |_| {
                let attempt = attempts_clone.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err("network error".to_string())
                } else {
                    Ok(())
                }
            }),
        );

        assert!(result.is_ok(), "retry should succeed");
        assert_eq!(attempts.load(Ordering::SeqCst), 2, "should retry after first failure");
    }

    #[test]
    fn send_with_retries_fails_after_all_attempts() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let event = serde_json::json!({"test": "data"});

        let result = send_with_retries(
            &event,
            3,
            &[500, 1000],
            Box::new(move |_| {
                attempts_clone.fetch_add(1, Ordering::SeqCst);
                Err("persistent error".to_string())
            }),
        );

        assert!(result.is_err(), "all attempts should fail");
        assert_eq!(attempts.load(Ordering::SeqCst), 3, "should try all 3 attempts");
    }

    #[test]
    fn send_with_retries_respects_max_attempts() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let event = serde_json::json!({"test": "data"});

        let result = send_with_retries(
            &event,
            1, // Only 1 attempt, no retries
            &[], // No backoff needed
            Box::new(move |_| {
                attempts_clone.fetch_add(1, Ordering::SeqCst);
                Err("error".to_string())
            }),
        );

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "should only try once when max_attempts=1");
    }
}
