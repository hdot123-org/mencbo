use tauri::{
    Manager, PhysicalPosition, PhysicalSize, Position, Size,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    WindowEvent, Emitter,
};
use serde_json::Value;

const PANEL_W: f64 = 360.0;
const GAP: f64 = 6.0;

/// Compute panel physical position from tray icon rect.
/// Panel right-edge aligns with tray icon right-edge; top sits below icon with gap.
fn compute_panel_position(
    rect_x: f64,
    rect_width: f64,
    rect_y: f64,
    rect_height: f64,
    scale: f64,
) -> (i32, i32) {
    let panel_w_phys = PANEL_W * scale;
    let x = rect_x + rect_width - panel_w_phys;
    let y = rect_y + rect_height + GAP * scale;
    (x as i32, y as i32)
}

/// Build the uv command arguments for running a daemon task.
/// Returns (program, args, cwd).
fn build_run_task_command(task_id: &str, uv_bin: &str, daemon_cwd: &str) -> (String, Vec<String>, String) {
    let program = uv_bin.to_string();
    let args = vec![
        "run".to_string(),
        "python".to_string(),
        "-m".to_string(),
        "mencbo".to_string(),
        "run-task".to_string(),
        task_id.to_string(),
    ];
    let cwd = daemon_cwd.to_string();
    (program, args, cwd)
}

/// Shared helper: read state.json from `path`, parse it, and transform from
/// daemon format (snake_case map) to frontend format (camelCase array).
///
/// This is the single source of truth for the payload shape emitted by both
/// `get_state` command and the `state-changed` watcher event, ensuring they
/// always produce identical structures.
///
/// On file-not-found or parse error, returns `{"mock": true, "tasks": []}`.
fn read_and_transform_state(path: &std::path::Path) -> Value {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(val) => {
                // Transform daemon format (tasks as map) to frontend format (tasks as array)
                let mut transformed = val.clone();
                if let Some(tasks_map) = val.get("tasks").and_then(|t| t.as_object()) {
                    let tasks_array: Vec<Value> = tasks_map
                        .iter()
                        .filter_map(|(task_id, task_data)| {
                            // Defensive: skip malformed entries where task_data is not an object
                            // (e.g., if daemon writes a string or number instead of an object).
                            // This prevents the watcher thread from panicking and stopping.
                            let task_obj = task_data.as_object()?;
                            let mut task = Value::Object(task_obj.clone());
                            // Add id field from the map key
                            task["id"] = Value::String(task_id.clone());
                            // Rename display_name -> name
                            if let Some(display_name) = task.get("display_name") {
                                task["name"] = display_name.clone();
                                task.as_object_mut().unwrap().remove("display_name");
                            }
                            // Rename last_run -> lastRun
                            if let Some(last_run) = task.get("last_run") {
                                task["lastRun"] = last_run.clone();
                                task.as_object_mut().unwrap().remove("last_run");
                            }
                            // Rename duration_ms -> durationMs
                            if let Some(duration_ms) = task.get("duration_ms") {
                                task["durationMs"] = duration_ms.clone();
                                task.as_object_mut().unwrap().remove("duration_ms");
                            }
                            Some(task)
                        })
                        .collect();
                    transformed["tasks"] = Value::Array(tasks_array);
                }
                transformed
            }
            Err(_) => serde_json::json!({ "mock": true, "tasks": [] }),
        },
        Err(_) => serde_json::json!({ "mock": true, "tasks": [] }),
    }
}

/// Resolve the path to state.json: MENCBO_STATE_PATH env var takes priority,
/// fallback to ~/.mencbo/state.json.
fn resolve_state_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("MENCBO_STATE_PATH") {
        std::path::PathBuf::from(p)
    } else {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(format!("{home}/.mencbo/state.json"))
    }
}

/// Resolve the log directory: MENCBO_LOG_DIR env var takes priority,
/// fallback to {tempdir}/mencbo — aligned with daemon's _DEFAULT_LOG_DIR
/// (example_tasks.py: ``Path(tempfile.gettempdir()) / "mencbo"``).
fn resolve_log_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("MENCBO_LOG_DIR") {
        std::path::PathBuf::from(d)
    } else {
        std::env::temp_dir().join("mencbo")
    }
}

/// Resolve the daemon working directory: MENCBO_DAEMON_DIR env var takes priority,
/// fallback to $HOME/mencbo/mencbo/apps/desktop/daemon.
fn resolve_daemon_dir() -> Result<String, String> {
    if let Ok(d) = std::env::var("MENCBO_DAEMON_DIR") {
        Ok(d)
    } else {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        Ok(format!("{home}/mencbo/mencbo/apps/desktop/daemon"))
    }
}

#[tauri::command]
fn get_state() -> Result<Value, String> {
    let path = resolve_state_path();
    Ok(read_and_transform_state(&path))
}

#[tauri::command]
async fn run_task(task: String) -> Result<(), String> {
    let uv_bin = std::env::var("MENCBO_UV_BIN")
        .unwrap_or_else(|_| "/opt/homebrew/bin/uv".to_string());

    let daemon_cwd = resolve_daemon_dir()?;

    let (program, args, cwd) = build_run_task_command(&task, &uv_bin, &daemon_cwd);

    tauri::async_runtime::spawn_blocking(move || {
        match std::process::Command::new(&program)
            .current_dir(&cwd)
            .args(&args)
            .spawn()
        {
            Ok(_) => {}
            Err(e) => {
                // Log spawn failure to stderr so it's visible in dev console / system log.
                // This makes missing uv or permission errors diagnosable instead of silent.
                eprintln!(
                    "[mencbo] run_task spawn failed (program={}, task={}): {}",
                    program, task, e
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn open_logs_dir() -> Result<(), String> {
    // Resolve log directory: MENCBO_LOG_DIR takes priority, fallback to {tempdir}/mencbo
    // (aligned with daemon's _DEFAULT_LOG_DIR in example_tasks.py:16)
    let log_dir = resolve_log_dir();
    
    // create_dir_all is idempotent and only creates if missing.
    // This ensures the directory exists before opening it in Finder.
    std::fs::create_dir_all(&log_dir).map_err(|e| format!("Failed to create log directory: {}", e))?;
    
    // Open the directory in Finder (macOS)
    std::process::Command::new("open")
        .arg(&log_dir)
        .spawn()
        .map_err(|e| format!("Failed to open log directory: {}", e))?;
    
    Ok(())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_state, run_task, open_logs_dir, quit_app])
        .setup(|app| {
            // Hide Dock icon (macOS only)
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Tray menu with quit item
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;

            let panel = app.get_webview_window("panel").expect("panel window");

            // Build tray icon in Rust (more reliable on macOS 26)
            // NOTE: tauri.conf.json must NOT contain app.trayIcon — that would
            // create a second NSStatusItem alongside this builder (dual tray bug).
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(move |tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        let Some(panel) = app.get_webview_window("panel") else {
                            return;
                        };

                        // Toggle: if visible, hide
                        if panel.is_visible().unwrap_or(false) {
                            let _ = panel.hide();
                            return;
                        }

                        // Position panel under tray icon using physical pixel rect
                        let scale = panel.scale_factor().unwrap_or(2.0);
                        
                        // Extract physical pixel values from rect
                        let rect_x = match rect.position {
                            Position::Physical(PhysicalPosition { x, .. }) => x as f64,
                            Position::Logical(logical_pos) => logical_pos.x * scale,
                        };
                        let rect_y = match rect.position {
                            Position::Physical(PhysicalPosition { y, .. }) => y as f64,
                            Position::Logical(logical_pos) => logical_pos.y * scale,
                        };
                        let rect_width = match rect.size {
                            Size::Physical(PhysicalSize { width, .. }) => width as f64,
                            Size::Logical(logical_size) => logical_size.width * scale,
                        };
                        let rect_height = match rect.size {
                            Size::Physical(PhysicalSize { height, .. }) => height as f64,
                            Size::Logical(logical_size) => logical_size.height * scale,
                        };
                        
                        let (x, y) = compute_panel_position(
                            rect_x,
                            rect_width,
                            rect_y,
                            rect_height,
                            scale,
                        );

                        // Order: set_position → show → set_focus
                        let _ = panel.set_position(Position::Physical(PhysicalPosition::new(x, y)));
                        let _ = panel.show();
                        let _ = panel.set_focus();
                    }
                })
                .build(app)?;

            // Window events: blur → hide, close request → hide (don't exit)
            let panel_for_blur = panel.clone();
            panel.on_window_event(move |e| match e {
                WindowEvent::Focused(false) => {
                    let _ = panel_for_blur.hide();
                }
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = panel_for_blur.hide();
                }
                _ => {}
            });

            // File watcher for state.json → emit state-changed.
            // FIX 4: Watch the *parent directory* instead of the file itself.
            // - No pre-write: we must NEVER write to ~/.mencbo/state.json at startup
            //   (红线 6: client is pure reader). The daemon is the sole writer.
            // - By watching the parent dir, we catch Create/Modify/Rename events
            //   from the daemon's atomic write pattern (os.replace).
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                use notify::{Watcher, RecursiveMode, Event, EventKind};

                let state_path = resolve_state_path();

                // Watch the parent directory (where the daemon does atomic replace).
                // The directory may or may not exist yet — watcher handles both.
                let watch_dir = match state_path.parent() {
                    Some(p) => p.to_path_buf(),
                    None => return,
                };

                // Do NOT create the state.json file here — client is read-only (红线 6).
                // The directory will be created below if needed (watcher requires it to exist).

                let handle = app_handle.clone();
                let path_clone = state_path.clone();
                let file_name = state_path.file_name().map(|n| n.to_os_string());

                let mut watcher = match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        // Only react to events that could affect our target file.
                        // We watch the parent dir, so we get events for all files
                        // in it; filter by file name to avoid spurious wakes.
                        let relevant = match &event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                // Check if any affected path matches our target filename
                                if let Some(ref target_name) = file_name {
                                    event.paths.iter().any(|p| {
                                        p.file_name().map(|n| n == target_name.as_os_str()).unwrap_or(false)
                                    })
                                } else {
                                    // If we can't determine the target filename, react to all
                                    true
                                }
                            }
                            _ => false,
                        };

                        if relevant {
                            // Use the shared transform helper — same shape as get_state
                            let payload = read_and_transform_state(&path_clone);
                            let _ = handle.emit("state-changed", payload);
                        }
                    }
                }) {
                    Ok(w) => w,
                    Err(_) => return,
                };

                // FIX (M1 scrutiny round-2): Ensure the watch directory exists
                // before mounting the watcher. On a pristine machine where
                // ~/.mencbo doesn't exist yet, the watcher would silently skip
                // and never detect the daemon's first atomic write. create_dir_all
                // creates the directory if missing (idempotent) — it never writes
                // state.json (红线 6: client is pure reader, daemon is sole writer).
                let _ = std::fs::create_dir_all(&watch_dir);

                let _ = watcher.watch(&watch_dir, RecursiveMode::NonRecursive);

                // Keep watcher alive
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error building tauri application")
        .run(|_handle, event| {
            // FIX 1: Exit guard — must check code.is_none() before prevent_exit().
            // tauri 2.11.5 prevent_exit() swallows ALL exit codes (including code:Some(0))
            // because runtime-wry skips ControlFlow::Exit on Prevent. Without the guard,
            // app.exit(0) from the quit menu / quit_app command is silently swallowed and
            // the process can never exit normally. See tauri issue #17723.
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serial lock for tests that mutate process-level env vars (MENCBO_STATE_PATH, etc.).
    /// cargo test runs tests in parallel by default; env vars are process-global, so
    /// concurrent set_var/remove_var across tests causes race conditions and flakes.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn panel_position_under_tray_rect() {
        // Simulate tray icon at right side of menu bar, retina scale=2
        let rect_x = 1780.0;
        let rect_width = 22.0;
        let rect_y = 0.0;
        let rect_height = 48.0; // physical pixels at 2x
        let scale = 2.0;

        let (x, y) = compute_panel_position(rect_x, rect_width, rect_y, rect_height, scale);

        // Panel width in physical = 360 * 2 = 720
        // x = 1780 + 22 - 720 = 1082
        assert_eq!(x, 1082);
        // y = 0 + 48 + 6*2 = 60
        assert_eq!(y, 60);
    }

    #[test]
    fn panel_position_clamped_to_monitor() {
        // Tray icon near left edge
        let rect_x = 100.0;
        let rect_width = 22.0;
        let rect_y = 0.0;
        let rect_height = 48.0;
        let scale = 2.0;

        let (x, y) = compute_panel_position(rect_x, rect_width, rect_y, rect_height, scale);

        // x = 100 + 22 - 720 = -598 (would be off-screen left, but no clamp in current impl)
        // This test documents current behavior
        assert_eq!(x, -598);
        assert_eq!(y, 60);
    }

    #[test]
    fn build_run_task_default_uv() {
        let (program, args, cwd) = build_run_task_command(
            "example:heartbeat",
            "/opt/homebrew/bin/uv",
            "/home/user/mencbo/apps/desktop/daemon",
        );
        assert_eq!(program, "/opt/homebrew/bin/uv");
        assert_eq!(args, vec!["run", "python", "-m", "mencbo", "run-task", "example:heartbeat"]);
        assert_eq!(cwd, "/home/user/mencbo/apps/desktop/daemon");
    }

    #[test]
    fn build_run_task_custom_uv() {
        let (program, args, _) = build_run_task_command(
            "test:task",
            "/custom/bin/uv",
            "/some/path",
        );
        assert_eq!(program, "/custom/bin/uv");
        assert_eq!(args, vec!["run", "python", "-m", "mencbo", "run-task", "test:task"]);
    }

    // ---- Serial env-mutating tests (use ENV_MUTEX) ----

    #[test]
    fn get_state_valid_json() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // VAL-STATE-001: Valid state.json is parsed with full object semantic equality.
        // The contract requires ALL fields preserved: updated_at, project_id, health,
        // tasks (with each task's display_name→name, last_run→lastRun, duration_ms→durationMs,
        // status, error — both null and non-null).
        let temp_dir = std::env::temp_dir().join("mencbo_test_valid_v3");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let state_path = temp_dir.join("state.json");

        // Fixture covers: 1 success task (error=null), 1 failed task (error=non-null),
        // 1 running task (duration_ms=null, error=null) — exercising all field variants.
        let valid_json = r#"{
            "updated_at": "2026-09-07T01:50:00+00:00",
            "project_id": "hdot123-org/mencbo",
            "health": "degraded",
            "tasks": {
                "example:heartbeat": {
                    "display_name": "示例·心跳日志",
                    "last_run": "2026-09-07T01:30:00+00:00",
                    "status": "success",
                    "duration_ms": 12,
                    "error": null
                },
                "example:failure-demo": {
                    "display_name": "示例·失败演示",
                    "last_run": "2026-09-07T00:50:00+00:00",
                    "status": "failed",
                    "duration_ms": 1234,
                    "error": "RuntimeError('演示失败')"
                },
                "example:maintenance": {
                    "display_name": "示例·维护日志",
                    "last_run": "2026-09-07T01:49:20+00:00",
                    "status": "running",
                    "duration_ms": null,
                    "error": null
                }
            }
        }"#;

        std::fs::write(&state_path, valid_json).unwrap();
        std::env::set_var("MENCBO_STATE_PATH", &state_path);

        let result = get_state().unwrap();

        // --- Top-level fields: full semantic equality ---
        // updated_at must be preserved (contract VAL-STATE-001)
        assert_eq!(result["updated_at"], "2026-09-07T01:50:00+00:00");
        // project_id must be preserved
        assert_eq!(result["project_id"], "hdot123-org/mencbo");
        // health must be preserved
        assert_eq!(result["health"], "degraded");

        // --- Tasks array (transformed from map) ---
        let tasks = result["tasks"].as_array().expect("tasks must be an array");
        assert_eq!(tasks.len(), 3);

        // Helper to find a task by id
        let find_task = |id: &str| -> &Value {
            tasks.iter().find(|t| t["id"] == id).expect("task not found")
        };

        // Success task: error is null, duration_ms=12→durationMs=12
        let heartbeat = find_task("example:heartbeat");
        assert_eq!(heartbeat["id"], "example:heartbeat");
        assert_eq!(heartbeat["name"], "示例·心跳日志");
        assert_eq!(heartbeat["lastRun"], "2026-09-07T01:30:00+00:00");
        assert_eq!(heartbeat["status"], "success");
        assert_eq!(heartbeat["durationMs"], 12);
        assert!(heartbeat["error"].is_null(), "success task error must be null");

        // Failed task: error is non-null string, duration_ms present
        let failure = find_task("example:failure-demo");
        assert_eq!(failure["id"], "example:failure-demo");
        assert_eq!(failure["name"], "示例·失败演示");
        assert_eq!(failure["lastRun"], "2026-09-07T00:50:00+00:00");
        assert_eq!(failure["status"], "failed");
        assert_eq!(failure["durationMs"], 1234);
        assert!(failure["error"].is_string(), "failed task error must be non-null string");
        assert_eq!(failure["error"], "RuntimeError('演示失败')");

        // Running task: duration_ms=null→durationMs=null, error=null
        let maintenance = find_task("example:maintenance");
        assert_eq!(maintenance["id"], "example:maintenance");
        assert_eq!(maintenance["name"], "示例·维护日志");
        assert_eq!(maintenance["status"], "running");
        assert!(maintenance["durationMs"].is_null(), "running task durationMs must be null");
        assert!(maintenance["error"].is_null(), "running task error must be null");

        // No snake_case keys should leak through (transformation check)
        for task in tasks {
            assert!(task.get("display_name").is_none(), "display_name should be renamed to name");
            assert!(task.get("last_run").is_none(), "last_run should be renamed to lastRun");
            assert!(task.get("duration_ms").is_none(), "duration_ms should be renamed to durationMs");
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn get_state_missing_file() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // VAL-STATE-002: Missing file returns mock marker
        let nonexistent = std::env::temp_dir().join("mencbo_test_nonexistent_v2_99999/state.json");
        std::env::set_var("MENCBO_STATE_PATH", &nonexistent);
        
        let result = get_state().unwrap();
        
        assert_eq!(result["mock"], true);
        assert!(result.get("tasks").unwrap().is_array());
        assert_eq!(result["tasks"].as_array().unwrap().len(), 0);
        
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn get_state_bad_json() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // VAL-STATE-003: Bad JSON returns mock marker
        let temp_dir = std::env::temp_dir().join("mencbo_test_bad_json_v2");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let state_path = temp_dir.join("state.json");
        
        std::fs::write(&state_path, "{ invalid json }}}").unwrap();
        std::env::set_var("MENCBO_STATE_PATH", &state_path);
        
        let result = get_state().unwrap();
        
        assert_eq!(result["mock"], true);
        assert!(result.get("tasks").unwrap().is_array());
        
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn get_state_truncated_json() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // VAL-STATE-003: Truncated JSON (partial write) returns mock marker
        let temp_dir = std::env::temp_dir().join("mencbo_test_truncated_v2");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let state_path = temp_dir.join("state.json");
        
        // Simulate atomic write in progress
        std::fs::write(&state_path, r#"{"updated_at": "2026-09-07"#).unwrap();
        std::env::set_var("MENCBO_STATE_PATH", &state_path);
        
        let result = get_state().unwrap();
        
        assert_eq!(result["mock"], true);
        
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn get_state_env_priority() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // VAL-STATE-001: MENCBO_STATE_PATH takes priority over default path
        let temp_dir = std::env::temp_dir().join("mencbo_test_env_priority_v2");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let env_path = temp_dir.join("env_state.json");
        
        let env_json = r#"{
            "updated_at": "2026-09-07T02:00:00+00:00",
            "project_id": "test/env-priority",
            "health": "degraded",
            "tasks": {}
        }"#;
        
        std::fs::write(&env_path, env_json).unwrap();
        std::env::set_var("MENCBO_STATE_PATH", &env_path);
        
        let result = get_state().unwrap();
        
        // Should read from env path, not default ~/.mencbo/state.json
        assert_eq!(result["project_id"], "test/env-priority");
        assert_eq!(result["health"], "degraded");
        
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn watcher_payload_matches_get_state_shape() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // VAL-STATE-005 (programmatic verification): The watcher callback uses
        // the same read_and_transform_state helper as get_state, so payloads
        // must be structurally identical.
        let temp_dir = std::env::temp_dir().join("mencbo_test_watcher_shape_v2");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let state_path = temp_dir.join("state.json");

        let valid_json = r#"{
            "updated_at": "2026-09-07T01:50:00+00:00",
            "project_id": "hdot123-org/mencbo",
            "health": "ok",
            "tasks": {
                "example:heartbeat": {
                    "display_name": "示例·心跳日志",
                    "last_run": "2026-09-07T01:30:00+00:00",
                    "status": "success",
                    "duration_ms": 12,
                    "error": null
                }
            }
        }"#;

        std::fs::write(&state_path, valid_json).unwrap();
        std::env::set_var("MENCBO_STATE_PATH", &state_path);

        // get_state result
        let from_command = get_state().unwrap();

        // read_and_transform_state result (what watcher emits)
        let from_watcher = read_and_transform_state(&state_path);

        // They must be structurally identical
        assert_eq!(from_command, from_watcher);

        // Both must have the correct frontend shape:
        // - tasks is an array (not a map)
        // - each task has id, name, lastRun, durationMs (camelCase)
        let tasks = from_watcher["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["id"], "example:heartbeat");
        assert_eq!(tasks[0]["name"], "示例·心跳日志");
        assert!(tasks[0].get("display_name").is_none());
        assert!(tasks[0].get("last_run").is_none());
        assert!(tasks[0].get("duration_ms").is_none());

        // Mock fallback also consistent
        let missing_path = temp_dir.join("nonexistent.json");
        let mock_from_helper = read_and_transform_state(&missing_path);
        assert_eq!(mock_from_helper["mock"], true);
        assert!(mock_from_helper.get("tasks").unwrap().is_array());
        assert_eq!(mock_from_helper["tasks"].as_array().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn run_task_daemon_dir_env_override() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // VAL-STATE-006: MENCBO_DAEMON_DIR env var overrides the default daemon cwd
        let custom_dir = "/custom/daemon/path";
        std::env::set_var("MENCBO_DAEMON_DIR", custom_dir);
        std::env::set_var("MENCBO_UV_BIN", "/custom/uv");

        let daemon_cwd = resolve_daemon_dir().unwrap();
        assert_eq!(daemon_cwd, custom_dir);

        let uv_bin = std::env::var("MENCBO_UV_BIN")
            .unwrap_or_else(|_| "/opt/homebrew/bin/uv".to_string());

        let (program, args, cwd) = build_run_task_command("test:task", &uv_bin, &daemon_cwd);
        assert_eq!(program, "/custom/uv");
        assert_eq!(cwd, "/custom/daemon/path");
        assert_eq!(args, vec!["run", "python", "-m", "mencbo", "run-task", "test:task"]);

        std::env::remove_var("MENCBO_DAEMON_DIR");
        std::env::remove_var("MENCBO_UV_BIN");
    }

    #[test]
    fn run_task_daemon_dir_default() {
        let _lock = ENV_MUTEX.lock().unwrap();

        // When MENCBO_DAEMON_DIR is not set, resolve_daemon_dir falls back to $HOME/...
        std::env::remove_var("MENCBO_DAEMON_DIR");

        let daemon_cwd = resolve_daemon_dir().unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(daemon_cwd, format!("{home}/mencbo/mencbo/apps/desktop/daemon"));
    }

    #[test]
    fn resolve_state_path_env_priority() {
        let _lock = ENV_MUTEX.lock().unwrap();

        let temp_dir = std::env::temp_dir().join("mencbo_test_resolve_path_v2");
        let _ = std::fs::create_dir_all(&temp_dir);
        let custom_path = temp_dir.join("custom_state.json");

        std::env::set_var("MENCBO_STATE_PATH", &custom_path);
        let resolved = resolve_state_path();
        assert_eq!(resolved, custom_path);

        std::env::remove_var("MENCBO_STATE_PATH");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resolve_log_dir_default() {
        // When MENCBO_LOG_DIR is not set, should return {tempdir}/mencbo
        // This aligns with daemon's _DEFAULT_LOG_DIR
        let _lock = ENV_MUTEX.lock().unwrap();
        
        // Remove the env var to test default behavior
        std::env::remove_var("MENCBO_LOG_DIR");
        
        let result = resolve_log_dir();
        let expected = std::env::temp_dir().join("mencbo");
        assert_eq!(result, expected);
    }

    #[test]
    fn resolve_log_dir_env_override() {
        // When MENCBO_LOG_DIR is set, it should take priority
        let _lock = ENV_MUTEX.lock().unwrap();
        
        let custom_path = std::env::temp_dir().join("custom_logs_test");
        std::env::set_var("MENCBO_LOG_DIR", &custom_path);
        
        let result = resolve_log_dir();
        assert_eq!(result, custom_path);
        
        std::env::remove_var("MENCBO_LOG_DIR");
    }

    #[test]
    fn read_and_transform_state_malformed_task_entry() {
        // Test that malformed task entries (non-object values) are skipped
        // instead of causing a panic. This is a defensive measure against
        // corrupted state.json files.
        let _lock = ENV_MUTEX.lock().unwrap();

        let temp_dir = std::env::temp_dir().join("mencbo_test_malformed_tasks");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let state_path = temp_dir.join("state.json");

        // Create state with mixed valid and malformed task entries
        let state_json = r#"{
            "updated_at": "2026-09-07T01:50:00+00:00",
            "project_id": "test/project",
            "health": "ok",
            "tasks": {
                "valid:task": {
                    "display_name": "Valid Task",
                    "last_run": "2026-09-07T01:30:00+00:00",
                    "status": "success",
                    "duration_ms": 100,
                    "error": null
                },
                "malformed:string": "this is not an object",
                "malformed:number": 42,
                "malformed:null": null,
                "malformed:array": [1, 2, 3]
            }
        }"#;

        std::fs::write(&state_path, state_json).unwrap();
        std::env::set_var("MENCBO_STATE_PATH", &state_path);

        // Should not panic and should only include the valid task
        let result = get_state().unwrap();
        
        // Verify we got the valid task
        let tasks = result["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1, "Should only contain the valid task");
        
        let valid_task = &tasks[0];
        assert_eq!(valid_task["id"], "valid:task");
        assert_eq!(valid_task["name"], "Valid Task");
        assert_eq!(valid_task["status"], "success");

        let _ = std::fs::remove_dir_all(&temp_dir);
        std::env::remove_var("MENCBO_STATE_PATH");
    }
}
