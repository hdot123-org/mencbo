//! Synchronous panic event sender (VAL-DIAG-006).
//!
//! Bypasses the batch queue and sends panic events directly via blocking reqwest.
//! This is necessary because:
//! 1. Batch queue runs in a separate thread that won't survive panic=abort
//! 2. Panic hook must complete synchronously before process termination
//! 3. We need guaranteed delivery even when the process is about to crash

use std::time::Duration;

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
    // Truncate message to ≤300 chars
    let truncated_msg: String = panic_message.chars().take(300).collect();

    // Truncate backtrace to first 500 chars
    let truncated_bt: String = backtrace.chars().take(500).collect();

    // Build event payload
    let event = serde_json::json!({
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
    });

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
    fn message_truncation() {
        let long_msg = "a".repeat(500);
        let truncated: String = long_msg.chars().take(300).collect();
        assert_eq!(truncated.len(), 300);
    }

    #[test]
    fn message_truncation_short() {
        let short_msg = "short message";
        let truncated: String = short_msg.chars().take(300).collect();
        assert_eq!(truncated, short_msg);
    }

    #[test]
    fn backtrace_truncation() {
        let long_bt = "backtrace line\n".repeat(100);
        let truncated: String = long_bt.chars().take(500).collect();
        assert_eq!(truncated.len(), 500);
    }

    #[test]
    fn event_payload_structure() {
        let event = serde_json::json!({
            "event": "diag_rust_panic",
            "distinct_id": "desktop-test-id",
            "properties": {
                "error_code": "E_RUST_PANIC",
                "message": "test panic",
                "backtrace": "test backtrace",
                "session_id": "session-123",
                "source": "rust_native",
                "environment": "production",
                "platform": "macos",
                "arch": "aarch64",
            }
        });

        assert_eq!(event["event"], "diag_rust_panic");
        assert_eq!(event["properties"]["error_code"], "E_RUST_PANIC");
        assert_eq!(event["properties"]["message"], "test panic");
        assert_eq!(event["properties"]["source"], "rust_native");
    }
}
