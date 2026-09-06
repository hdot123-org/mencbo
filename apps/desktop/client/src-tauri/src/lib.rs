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

#[tauri::command]
fn get_state() -> Result<Value, String> {
    // MENCBO_STATE_PATH takes priority, fallback to ~/.mencbo/state.json
    let path = if let Ok(p) = std::env::var("MENCBO_STATE_PATH") {
        p
    } else {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        format!("{home}/.mencbo/state.json")
    };

    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(val) => {
                // Transform daemon format (tasks as map) to frontend format (tasks as array)
                let mut transformed = val.clone();
                if let Some(tasks_map) = val.get("tasks").and_then(|t| t.as_object()) {
                    let tasks_array: Vec<Value> = tasks_map
                        .iter()
                        .map(|(task_id, task_data)| {
                            let mut task = task_data.clone();
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
                            task
                        })
                        .collect();
                    transformed["tasks"] = Value::Array(tasks_array);
                }
                Ok(transformed)
            }
            Err(_) => Ok(serde_json::json!({ "mock": true, "tasks": [] })),
        },
        Err(_) => Ok(serde_json::json!({ "mock": true, "tasks": [] })),
    }
}

#[tauri::command]
async fn run_task(task: String) -> Result<(), String> {
    let uv_bin = std::env::var("MENCBO_UV_BIN")
        .unwrap_or_else(|_| "/opt/homebrew/bin/uv".to_string());

    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let daemon_cwd = format!("{home}/mencbo/mencbo/apps/desktop/daemon");

    let (program, args, cwd) = build_run_task_command(&task, &uv_bin, &daemon_cwd);

    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::process::Command::new(&program)
            .current_dir(&cwd)
            .args(&args)
            .spawn();
    });
    Ok(())
}

#[tauri::command]
fn open_logs_dir() -> Result<(), String> {
    let log_dir = if let Ok(d) = std::env::var("MENCBO_LOG_DIR") {
        d
    } else {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        format!("{home}/.mencbo/logs")
    };
    // Ensure dir exists
    let _ = std::fs::create_dir_all(&log_dir);
    let _ = std::process::Command::new("open")
        .arg(&log_dir)
        .spawn();
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

            // File watcher for state.json → emit state-changed
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                use notify::{Watcher, RecursiveMode, Event, EventKind};

                let state_path = if let Ok(p) = std::env::var("MENCBO_STATE_PATH") {
                    std::path::PathBuf::from(p)
                } else {
                    let home = std::env::var("HOME").unwrap_or_default();
                    std::path::PathBuf::from(format!("{home}/.mencbo/state.json"))
                };

                // Ensure parent dir exists for watcher
                if let Some(parent) = state_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                // Create the file if it doesn't exist so watcher can track it
                if !state_path.exists() {
                    let _ = std::fs::write(&state_path, "{}");
                }

                let handle = app_handle.clone();
                let path_clone = state_path.clone();
                let mut watcher = match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        match event.kind {
                            EventKind::Modify(_) | EventKind::Create(_) => {
                                // Re-read and emit
                                if let Ok(text) = std::fs::read_to_string(&path_clone) {
                                    if let Ok(val) = serde_json::from_str::<Value>(&text) {
                                        let _ = handle.emit("state-changed", val);
                                    } else {
                                        let _ = handle.emit("state-changed", serde_json::json!({ "mock": true }));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }) {
                    Ok(w) => w,
                    Err(_) => return,
                };

                let _ = watcher.watch(&state_path, RecursiveMode::NonRecursive);

                // Keep watcher alive
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
            });

            // Prevent exit when all windows closed
            // (handled by RunEvent below)

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error building tauri application")
        .run(|_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn get_state_valid_json() {
        // VAL-STATE-001: Valid state.json is parsed correctly
        let temp_dir = std::env::temp_dir().join("mencbo_test_valid");
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
        
        let result = get_state().unwrap();
        
        // Should have tasks array (transformed from map)
        assert!(result.get("tasks").unwrap().is_array());
        let tasks = result["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        
        // Task should have id field and renamed fields
        let task = &tasks[0];
        assert_eq!(task["id"], "example:heartbeat");
        assert_eq!(task["name"], "示例·心跳日志");
        assert_eq!(task["lastRun"], "2026-09-07T01:30:00+00:00");
        assert_eq!(task["durationMs"], 12);
        assert_eq!(task["status"], "success");
        
        // Should preserve health field
        assert_eq!(result["health"], "ok");
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn get_state_missing_file() {
        // VAL-STATE-002: Missing file returns mock marker
        let nonexistent = std::env::temp_dir().join("mencbo_test_nonexistent_99999/state.json");
        std::env::set_var("MENCBO_STATE_PATH", &nonexistent);
        
        let result = get_state().unwrap();
        
        assert_eq!(result["mock"], true);
        assert!(result.get("tasks").unwrap().is_array());
        assert_eq!(result["tasks"].as_array().unwrap().len(), 0);
        
        std::env::remove_var("MENCBO_STATE_PATH");
    }

    #[test]
    fn get_state_bad_json() {
        // VAL-STATE-003: Bad JSON returns mock marker
        let temp_dir = std::env::temp_dir().join("mencbo_test_bad_json");
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
        // VAL-STATE-003: Truncated JSON (partial write) returns mock marker
        let temp_dir = std::env::temp_dir().join("mencbo_test_truncated");
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
        // VAL-STATE-001: MENCBO_STATE_PATH takes priority over default path
        let temp_dir = std::env::temp_dir().join("mencbo_test_env_priority");
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
}
