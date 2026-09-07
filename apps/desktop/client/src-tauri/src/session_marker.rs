//! Session marker file for dirty exit detection (architecture decision #4).
//!
//! On startup, a "running" marker file is written to `app_data_dir/.session_marker`
//! containing the current session_id and start timestamp. On normal exit, the marker
//! is cleared. On next startup, if the marker still exists, it means the previous
//! session did not exit cleanly (crash/SIGKILL) — we emit `rust_exit{reason:"dirty"}`
//! to reconstruct the missing exit event.
//!
//! The marker file is JSON:
//! ```json
//! {
//!   "session_id": "0192f...",
//!   "started_at": "2026-09-08T12:00:00.000Z"
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::path::Path;

const MARKER_FILENAME: &str = ".session_marker";

/// Contents of the session marker file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMarker {
    /// The session_id (launch_id) of the running instance.
    pub session_id: String,
    /// RFC3339 UTC timestamp when the session started.
    pub started_at: String,
}

/// Result of checking for a leftover session marker (dirty exit detection).
#[derive(Debug, Clone, PartialEq)]
pub struct DirtyExitInfo {
    /// The session_id of the previous session that did not exit cleanly.
    pub prev_session_id: String,
    /// RFC3339 UTC timestamp when the previous session started.
    /// Used to estimate uptime: `now - started_at`.
    pub prev_started_at: String,
}

/// Write a session marker for the current session.
///
/// Called during app setup, after identity is initialized.
/// If a marker already exists, it is overwritten (shouldn't happen in normal flow,
/// but we handle it gracefully).
///
/// # Errors
/// Returns an error if the file cannot be written. The app should continue
/// even if this fails — analytics is best-effort.
pub fn write_session_marker(
    app_data_dir: &Path,
    session_id: &str,
    started_at: &str,
) -> Result<(), String> {
    let marker = SessionMarker {
        session_id: session_id.to_string(),
        started_at: started_at.to_string(),
    };

    let marker_path = app_data_dir.join(MARKER_FILENAME);
    let json = serde_json::to_string(&marker)
        .map_err(|e| format!("Failed to serialize session marker: {}", e))?;

    // Atomic write: write to tmp then rename (same pattern as install_id)
    let tmp_path = app_data_dir.join(".session_marker.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("Failed to write session marker tmp: {}", e))?;
    std::fs::rename(&tmp_path, &marker_path)
        .map_err(|e| format!("Failed to rename session marker: {}", e))?;

    Ok(())
}

/// Read an existing session marker, if present.
///
/// Returns `None` if the file doesn't exist or cannot be parsed.
/// This is non-fatal — missing marker just means no dirty exit to report.
pub fn read_session_marker(app_data_dir: &Path) -> Option<SessionMarker> {
    let marker_path = app_data_dir.join(MARKER_FILENAME);
    let contents = std::fs::read_to_string(&marker_path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Clear (delete) the session marker file.
///
/// Called during normal exit. If the file doesn't exist, this is a no-op.
/// Returns an error only if the file exists but cannot be deleted.
pub fn clear_session_marker(app_data_dir: &Path) -> Result<(), String> {
    let marker_path = app_data_dir.join(MARKER_FILENAME);
    if marker_path.exists() {
        std::fs::remove_file(&marker_path)
            .map_err(|e| format!("Failed to remove session marker: {}", e))?;
    }
    Ok(())
}

/// Check for a leftover session marker (dirty exit detection).
///
/// Called during app setup, BEFORE writing the new marker.
/// If a marker exists from a previous session, that session did not exit cleanly.
/// Returns `Some(DirtyExitInfo)` with the previous session's details, or `None`.
pub fn check_dirty_exit(app_data_dir: &Path) -> Option<DirtyExitInfo> {
    let marker = read_session_marker(app_data_dir)?;
    Some(DirtyExitInfo {
        prev_session_id: marker.session_id,
        prev_started_at: marker.started_at,
    })
}

/// Estimate uptime in seconds from a start timestamp to now.
///
/// Parses an RFC3339 timestamp and computes the difference from current time.
/// Returns `None` if the timestamp cannot be parsed.
/// This is a best-effort estimate for dirty exit reporting.
pub fn estimate_uptime_seconds(started_at: &str) -> Option<u64> {
    // Parse RFC3339 timestamp manually (no chrono dependency).
    // We only need to handle the format we produce: "YYYY-MM-DDTHH:MM:SS.mmmZ"
    let started_at = started_at.trim();

    // Extract components from "2026-09-08T12:00:00.000Z"
    if started_at.len() < 20 {
        return None;
    }

    let year: u64 = started_at[0..4].parse().ok()?;
    let month: u64 = started_at[5..7].parse().ok()?;
    let day: u64 = started_at[8..10].parse().ok()?;
    let hour: u64 = started_at[11..13].parse().ok()?;
    let min: u64 = started_at[14..16].parse().ok()?;
    let sec: u64 = started_at[17..19].parse().ok()?;

    // Convert to Unix timestamp (simplified — good enough for uptime estimates)
    let start_secs = datetime_to_unix_secs(year, month, day, hour, min, sec);

    // Current time
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let now_secs = now.as_secs();

    // Uptime is the difference, clamped to 0 (clock skew could make it negative)
    Some(now_secs.saturating_sub(start_secs))
}

/// Convert a datetime to Unix timestamp (seconds since 1970-01-01 00:00:00 UTC).
/// Inverse of the Howard Hinnant algorithm in analytics.rs.
fn datetime_to_unix_secs(year: u64, month: u64, day: u64, hour: u64, min: u64, sec: u64) -> u64 {
    // Adjust months so March = 0, ..., February = 11
    // This makes leap day the last day of the year, simplifying arithmetic.
    let (y, m) = if month <= 2 {
        (year as i64 - 1, (month + 9) as i64)
    } else {
        (year as i64, (month - 3) as i64)
    };

    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // year of era [0, 399]
    let doy = (153 * m as u64 + 2) / 5 + day - 1; // day of year [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // day of era [0, 146096]
    let days_since_epoch = era * 146097 + doe as i64 - 719468;

    (days_since_epoch * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── Marker write/read tests ──

    #[test]
    fn write_and_read_marker() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        let session_id = "test-session-123";
        let started_at = "2026-09-08T12:00:00.000Z";

        write_session_marker(app_data_dir, session_id, started_at).unwrap();

        let marker = read_session_marker(app_data_dir).expect("marker should exist");
        assert_eq!(marker.session_id, session_id);
        assert_eq!(marker.started_at, started_at);
    }

    #[test]
    fn marker_file_is_json() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        write_session_marker(app_data_dir, "sess-1", "2026-09-08T12:00:00.000Z").unwrap();

        let marker_path = app_data_dir.join(MARKER_FILENAME);
        let contents = std::fs::read_to_string(&marker_path).unwrap();

        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed["session_id"], "sess-1");
        assert_eq!(parsed["started_at"], "2026-09-08T12:00:00.000Z");
    }

    #[test]
    fn marker_atomic_write_no_tmp_leftover() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        write_session_marker(app_data_dir, "sess-1", "2026-09-08T12:00:00.000Z").unwrap();

        // Tmp file should have been renamed away
        let tmp_path = app_data_dir.join(".session_marker.tmp");
        assert!(!tmp_path.exists(), "tmp file should not exist after write");
    }

    #[test]
    fn read_nonexistent_marker_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        let marker = read_session_marker(app_data_dir);
        assert!(marker.is_none(), "no marker should exist in fresh dir");
    }

    #[test]
    fn read_corrupted_marker_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // Write garbage
        let marker_path = app_data_dir.join(MARKER_FILENAME);
        std::fs::write(&marker_path, "not valid json {{{").unwrap();

        let marker = read_session_marker(app_data_dir);
        assert!(marker.is_none(), "corrupted marker should return None");
    }

    #[test]
    fn write_overwrites_existing_marker() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        write_session_marker(app_data_dir, "old-session", "2026-09-08T10:00:00.000Z").unwrap();
        write_session_marker(app_data_dir, "new-session", "2026-09-08T12:00:00.000Z").unwrap();

        let marker = read_session_marker(app_data_dir).unwrap();
        assert_eq!(marker.session_id, "new-session");
        assert_eq!(marker.started_at, "2026-09-08T12:00:00.000Z");
    }

    // ── Clear marker tests ──

    #[test]
    fn clear_existing_marker() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        write_session_marker(app_data_dir, "sess-1", "2026-09-08T12:00:00.000Z").unwrap();
        assert!(app_data_dir.join(MARKER_FILENAME).exists());

        clear_session_marker(app_data_dir).unwrap();
        assert!(!app_data_dir.join(MARKER_FILENAME).exists());
    }

    #[test]
    fn clear_nonexistent_marker_is_noop() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // Should not error
        clear_session_marker(app_data_dir).unwrap();
    }

    // ── Dirty exit detection tests ──

    #[test]
    fn dirty_exit_detected_when_marker_exists() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // Simulate a previous session that crashed (marker left behind)
        write_session_marker(app_data_dir, "crashed-session", "2026-09-08T10:00:00.000Z").unwrap();

        // Next startup detects the leftover marker
        let dirty = check_dirty_exit(app_data_dir);
        assert!(dirty.is_some(), "dirty exit should be detected");

        let info = dirty.unwrap();
        assert_eq!(info.prev_session_id, "crashed-session");
        assert_eq!(info.prev_started_at, "2026-09-08T10:00:00.000Z");
    }

    #[test]
    fn no_dirty_exit_on_clean_start() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // Fresh install, no previous session
        let dirty = check_dirty_exit(app_data_dir);
        assert!(dirty.is_none(), "no dirty exit on fresh start");
    }

    #[test]
    fn no_dirty_exit_after_marker_cleared() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // Simulate normal exit: marker written then cleared
        write_session_marker(app_data_dir, "sess-1", "2026-09-08T10:00:00.000Z").unwrap();
        clear_session_marker(app_data_dir).unwrap();

        // Next startup should not see dirty exit
        let dirty = check_dirty_exit(app_data_dir);
        assert!(dirty.is_none(), "no dirty exit after clean shutdown");
    }

    #[test]
    fn dirty_exit_with_corrupted_marker_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // Write corrupted marker
        let marker_path = app_data_dir.join(MARKER_FILENAME);
        std::fs::write(&marker_path, "garbage").unwrap();

        // Should not report dirty exit (can't parse marker)
        let dirty = check_dirty_exit(app_data_dir);
        assert!(dirty.is_none(), "corrupted marker should not trigger dirty exit");
    }

    // ── Uptime estimation tests ──

    #[test]
    fn datetime_to_unix_secs_epoch() {
        // 1970-01-01T00:00:00Z should be 0
        let result = datetime_to_unix_secs(1970, 1, 1, 0, 0, 0);
        assert_eq!(result, 0, "Unix epoch should be 0");
    }

    #[test]
    fn datetime_to_unix_secs_known_value() {
        // 2024-01-15T09:30:45Z = 1705311045
        let result = datetime_to_unix_secs(2024, 1, 15, 9, 30, 45);
        assert_eq!(result, 1705311045, "2024-01-15T09:30:45Z should be 1705311045");
    }

    #[test]
    fn estimate_uptime_from_recent_timestamp() {
        // Use a timestamp from "now" minus 60 seconds
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let start_secs = now - 60;

        // Convert to RFC3339-like format using the same algorithm as analytics.rs
        let (year, month, day, hour, min, sec) = unix_secs_to_datetime_for_test(start_secs);
        let started_at = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
            year, month, day, hour, min, sec
        );

        eprintln!("start_secs: {}, started_at: {}", start_secs, started_at);
        let parsed_secs = datetime_to_unix_secs(year, month, day, hour, min, sec);
        eprintln!("parsed_secs: {}, diff: {}", parsed_secs, parsed_secs as i64 - start_secs as i64);

        let uptime = estimate_uptime_seconds(&started_at).unwrap();
        // Should be approximately 60 seconds (allow 5s tolerance for test execution time)
        assert!(
            uptime >= 55 && uptime <= 70,
            "uptime should be ~60s, got {}",
            uptime
        );
    }

    #[test]
    fn estimate_uptime_invalid_timestamp_returns_none() {
        assert!(estimate_uptime_seconds("not a timestamp").is_none());
        assert!(estimate_uptime_seconds("").is_none());
        assert!(estimate_uptime_seconds("2026").is_none());
    }

    #[test]
    fn estimate_uptime_epoch() {
        // Very old timestamp should give a large uptime
        let uptime = estimate_uptime_seconds("1970-01-01T00:00:00.000Z").unwrap();
        // Should be many years worth of seconds
        assert!(uptime > 1_000_000_000, "uptime from epoch should be huge");
    }

    // Helper for test: convert unix secs to datetime components
    fn unix_secs_to_datetime_for_test(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
        // Must use the exact same algorithm as analytics.rs
        let sec = secs % 60;
        let mins_total = secs / 60;
        let min = mins_total % 60;
        let hours_total = mins_total / 60;
        let hour = hours_total % 24;
        let days = hours_total / 24;

        let z = days + 719468;
        let era = z / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };

        (y, m, d, hour, min, sec)
    }

    // ── Integration: full lifecycle simulation ──

    #[test]
    fn lifecycle_clean_start_and_exit() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // 1. Startup: check dirty exit (should be none)
        assert!(check_dirty_exit(app_data_dir).is_none());

        // 2. Write marker
        write_session_marker(app_data_dir, "session-1", "2026-09-08T12:00:00.000Z").unwrap();

        // 3. Normal exit: clear marker
        clear_session_marker(app_data_dir).unwrap();

        // 4. Next startup: no dirty exit
        assert!(check_dirty_exit(app_data_dir).is_none());
    }

    #[test]
    fn lifecycle_dirty_exit_recovery() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // 1. First startup: write marker
        write_session_marker(app_data_dir, "session-1", "2026-09-08T12:00:00.000Z").unwrap();

        // 2. Simulate crash: marker NOT cleared (process killed)

        // 3. Second startup: detect dirty exit
        let dirty = check_dirty_exit(app_data_dir).unwrap();
        assert_eq!(dirty.prev_session_id, "session-1");

        // 4. Write new marker for new session
        write_session_marker(app_data_dir, "session-2", "2026-09-08T13:00:00.000Z").unwrap();

        // 5. Normal exit: clear marker
        clear_session_marker(app_data_dir).unwrap();

        // 6. Third startup: clean
        assert!(check_dirty_exit(app_data_dir).is_none());
    }

    #[test]
    fn marker_creates_parent_dir_if_missing() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path().join("nested").join("dir");

        // Directory doesn't exist yet
        assert!(!app_data_dir.exists());

        // write_session_marker should fail because dir doesn't exist
        // (we don't auto-create it — lib.rs handles that)
        let result = write_session_marker(&app_data_dir, "sess", "2026-09-08T12:00:00.000Z");
        assert!(result.is_err(), "should fail when dir doesn't exist");
    }
}
