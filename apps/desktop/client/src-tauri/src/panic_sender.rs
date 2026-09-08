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
/// - `properties` with error_code, message (truncated), backtrace (truncated), session_id, source, environment, platform, arch
pub(crate) fn build_panic_event(
    api_key: &str,
    install_id: &str,
    session_id: &str,
    panic_message: &str,
    backtrace: &str,
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
        }
    })
}

/// Send a panic event synchronously to PostHog.
///
/// This function:
/// - Truncates message to ≤300 chars (PostHog property limit)
/// - Extracts backtrace summary (first 500 chars)
/// - Sends via blocking HTTP POST (bypasses batch queue)
/// - Uses a short timeout (3s) to avoid hanging the panic handler
///
/// Called from std::panic::set_hook — must be synchronous and fast.
pub(crate) fn send_panic_event(
    api_key: &str,
    host: &str,
    install_id: &str,
    session_id: &str,
    panic_message: &str,
    backtrace: &str,
) {
    let event = build_panic_event(api_key, install_id, session_id, panic_message, backtrace);

    // Send via blocking POST (bypass batch queue)
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

    match client.post(&url).json(&event).send() {
        Ok(resp) => {
            if resp.status().is_success() {
                eprintln!("[panic_sender] panic event sent successfully");
            } else {
                eprintln!(
                    "[panic_sender] panic event send failed: HTTP {}",
                    resp.status()
                );
            }
        }
        Err(e) => {
            eprintln!("[panic_sender] panic event send failed: {}", e);
        }
    }
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
        );

        let message = event["properties"]["message"].as_str().unwrap();
        assert_eq!(message.chars().count(), 300, "must truncate by char count, not byte count");
    }
}
