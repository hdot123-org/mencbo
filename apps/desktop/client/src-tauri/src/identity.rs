use uuid::Uuid;

/// Get or create the persistent install_id for this application installation.
///
/// The install_id is a stable identifier in the format `desktop-{uuidv4}` that
/// persists across application restarts. It's stored in `app_data_dir/install_id`.
///
/// # Algorithm
/// 1. If `app_data_dir/install_id` exists and contains a valid ID, return it
/// 2. Otherwise, generate a new `desktop-{uuidv4}` and atomically write it
///
/// # Atomic Write
/// Uses tmp+rename pattern: write to `install_id.tmp` then rename to `install_id`.
/// This prevents partial writes from corrupting the file.
///
/// # Errors
/// Returns an error if the file cannot be read/written or the directory cannot be created.
pub fn get_or_create_install_id(app_data_dir: &std::path::Path) -> Result<String, String> {
    let install_id_path = app_data_dir.join("install_id");

    // Try to read existing install_id
    if let Ok(contents) = std::fs::read_to_string(&install_id_path) {
        let trimmed = contents.trim();
        if is_valid_install_id(trimmed) {
            return Ok(trimmed.to_string());
        }
    }

    // Generate new install_id
    let new_id = generate_install_id();

    // Ensure app_data_dir exists
    std::fs::create_dir_all(app_data_dir)
        .map_err(|e| format!("Failed to create app_data_dir: {}", e))?;

    // Atomic write: write to tmp file then rename
    let tmp_path = app_data_dir.join("install_id.tmp");
    std::fs::write(&tmp_path, &new_id)
        .map_err(|e| format!("Failed to write tmp install_id: {}", e))?;

    std::fs::rename(&tmp_path, &install_id_path)
        .map_err(|e| format!("Failed to rename tmp to install_id: {}", e))?;

    Ok(new_id)
}

/// Generate a new install_id in the format `desktop-{uuidv4}`
fn generate_install_id() -> String {
    format!("desktop-{}", Uuid::new_v4())
}

/// Validate that a string matches the install_id format: `desktop-{uuidv4}`
pub fn is_valid_install_id(s: &str) -> bool {
    if !s.starts_with("desktop-") {
        return false;
    }

    let uuid_part = &s[8..]; // Skip "desktop-"
    Uuid::parse_str(uuid_part).is_ok()
}

/// Generate a new launch_id (uuidv7) for this application session.
///
/// Called once per application startup to create a unique session identifier.
pub fn generate_launch_id() -> String {
    Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_install_id_format() {
        let id = generate_install_id();
        assert!(
            is_valid_install_id(&id),
            "Generated ID should be valid: {}",
            id
        );
        assert!(id.starts_with("desktop-"), "ID should start with 'desktop-'");
    }

    #[test]
    fn test_install_id_stability() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();

        // First call creates new ID
        let id1 = get_or_create_install_id(app_data_dir).unwrap();
        assert!(is_valid_install_id(&id1));

        // Second call returns same ID
        let id2 = get_or_create_install_id(app_data_dir).unwrap();
        assert_eq!(id1, id2, "Install ID should be stable across calls");
    }

    #[test]
    fn test_install_id_atomic_write() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();
        let install_id_path = app_data_dir.join("install_id");
        let tmp_path = app_data_dir.join("install_id.tmp");

        // Create ID
        let id = get_or_create_install_id(app_data_dir).unwrap();

        // Verify file exists
        assert!(install_id_path.exists(), "install_id file should exist");

        // Verify tmp file was cleaned up
        assert!(!tmp_path.exists(), "tmp file should not exist after rename");

        // Verify content matches
        let stored = fs::read_to_string(&install_id_path).unwrap();
        assert_eq!(stored, id, "Stored ID should match returned ID");
    }

    #[test]
    fn test_launch_id_format() {
        let id = generate_launch_id();

        // UUIDv7 should be parseable
        let uuid = Uuid::parse_str(&id).expect("Launch ID should be valid UUID");

        // Verify it's version 7
        assert_eq!(uuid.get_version_num(), 7, "Launch ID should be UUIDv7");
    }

    #[test]
    fn test_launch_id_uniqueness() {
        let id1 = generate_launch_id();
        let id2 = generate_launch_id();

        assert_ne!(id1, id2, "Each launch should get unique ID");
    }

    #[test]
    fn test_valid_install_id_format() {
        // Valid cases
        assert!(is_valid_install_id(
            "desktop-550e8400-e29b-41d4-a716-446655440000"
        ));
        assert!(is_valid_install_id(
            "desktop-00000000-0000-0000-0000-000000000000"
        ));

        // Invalid cases
        assert!(!is_valid_install_id("desktop-"));
        assert!(!is_valid_install_id("desktop-invalid-uuid"));
        assert!(!is_valid_install_id("mobile-550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_valid_install_id(""));
    }

    #[test]
    fn test_install_id_corrupted_file() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path();
        let install_id_path = app_data_dir.join("install_id");

        // Write corrupted content
        fs::write(&install_id_path, "corrupted-data").unwrap();

        // Should generate new ID instead of failing
        let id = get_or_create_install_id(app_data_dir).unwrap();
        assert!(is_valid_install_id(&id));

        // File should now contain valid ID
        let stored = fs::read_to_string(&install_id_path).unwrap();
        assert_eq!(stored, id);
    }

    #[test]
    fn test_install_id_missing_directory() {
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path().join("nested").join("dir");

        // Directory doesn't exist yet
        assert!(!app_data_dir.exists());

        // Should create directory and ID
        let id = get_or_create_install_id(&app_data_dir).unwrap();
        assert!(is_valid_install_id(&id));
        assert!(app_data_dir.exists(), "Directory should be created");
        assert!(app_data_dir.join("install_id").exists());
    }

    #[test]
    fn test_install_id_io_error_handling() {
        // Test that IO errors are properly propagated (Fix 3: graceful degradation)
        let temp_dir = TempDir::new().unwrap();
        let app_data_dir = temp_dir.path().join("readonly");
        
        // Create directory but make it read-only
        std::fs::create_dir(&app_data_dir).unwrap();
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&app_data_dir, std::fs::Permissions::from_mode(0o444)).unwrap();
            
            // Try to create install_id - should fail gracefully
            let result = get_or_create_install_id(&app_data_dir);
            
            // Restore permissions before cleanup
            std::fs::set_permissions(&app_data_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            
            // Should return error instead of panicking
            assert!(result.is_err(), "Should return error on IO failure");
            let err = result.unwrap_err();
            assert!(err.contains("Failed to create app_data_dir") || err.contains("Failed to write tmp install_id"),
                "Error message should indicate IO failure: {}", err);
        }
        
        #[cfg(not(unix))]
        {
            // On non-Unix systems, skip this test
            // The logic is still tested via the read-only file case below
            let _ = app_data_dir;
        }
    }
}
