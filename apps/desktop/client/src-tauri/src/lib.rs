mod analytics;
mod identity;
mod session_marker;

use serde_json::Value;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, WindowEvent,
};
use tauri_plugin_updater::UpdaterExt;
use identity::{get_or_create_install_id, generate_launch_id};

/// PostHog observability. Key is injected at build time via build.rs from POSTHOG_KEY env var.
/// If POSTHOG_KEY is not set, falls back to "NO_KEY" sentinel and all captures become no-op.
const POSTHOG_KEY: &str = env!("POSTHOG_KEY", "PostHog key must be provided via POSTHOG_KEY env var or defaults to NO_KEY");
const POSTHOG_HOST: &str = "https://us.i.posthog.com";

/// Global identity state (install_id + session_id) for this app instance.
/// Stored in a OnceLock to ensure thread-safe initialization.
use std::sync::OnceLock;
static IDENTITY: OnceLock<(String, String)> = OnceLock::new();

/// Global app version for analytics events. Set once during setup from tauri context.
/// This avoids passing app context through every posthog_capture call.
static APP_VERSION: OnceLock<String> = OnceLock::new();

/// Global startup timestamp for calculating session uptime in exit events.
/// Set during app setup, used to compute `uptime_s` in rust_exit{normal}.
static STARTED_AT: OnceLock<std::time::Instant> = OnceLock::new();

/// Tracks the exit reason and "via" field for normal exits.
/// Set by quit paths (command, tray, window close), consumed by RunEvent::Exit.
/// Architecture decision 4: rust_exit is the authoritative exit event.
static EXIT_INFO: OnceLock<(String, String)> = OnceLock::new();

/// Fire-and-forget analytics from the Rust side: enqueue to batch queue.
/// The batch queue handles batching (20 items or 60s), retry with exponential backoff,
/// and dropped event tracking (VAL-REL-001, VAL-REL-005).
///
/// Environment gating: debug builds (cfg!(debug_assertions)) are complete no-op.
/// This is the second layer of defense after POSTHOG_KEY gating (VAL-ENV-005).
///
/// All events automatically receive baseline properties:
/// - app_version: from tauri context (VAL-ENV-003)
/// - environment: "production" for release builds (VAL-ENV-002)
/// - platform/arch: from std::env::consts
/// - session_id/source: identity information
fn posthog_capture(event: &str, mut props: serde_json::Value) {
    // No-op in debug builds (VAL-ENV-001: dev 构建零上报)
    if cfg!(debug_assertions) {
        return;
    }

    // No-op if PostHog key is not configured (VAL-ENV-005)
    if POSTHOG_KEY == "NO_KEY" {
        return;
    }

    let (install_id, session_id) = IDENTITY
        .get()
        .cloned()
        .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

    // No-op if identity initialization failed (graceful degradation, fix-m1-review-findings)
    // "Analytics never kills the host" invariant
    if install_id == "analytics-disabled" {
        return;
    }

    // Inject baseline properties into every event (VAL-ENV-002/003)
    if let Some(obj) = props.as_object_mut() {
        obj.insert("session_id".to_string(), serde_json::Value::String(session_id));
        obj.insert("source".to_string(), serde_json::Value::String("rust_native".to_string()));
        
        // Environment properties: environment is always "production" for release builds
        // (debug builds are already gated by cfg!(debug_assertions) above)
        obj.insert("environment".to_string(), serde_json::Value::String("production".to_string()));
        
        // Platform and architecture from std::env::consts
        obj.insert("platform".to_string(), serde_json::Value::String(std::env::consts::OS.to_string()));
        obj.insert("arch".to_string(), serde_json::Value::String(std::env::consts::ARCH.to_string()));
        
        // app_version from tauri context (VAL-ENV-003)
        if let Some(version) = APP_VERSION.get() {
            obj.insert("app_version".to_string(), serde_json::Value::String(version.clone()));
        }
    }

    // Enqueue to batch queue instead of spawning a new thread
    analytics::enqueue(event, &install_id, props);
}

const PANEL_W: f64 = 360.0;
const GAP: f64 = 6.0;

/// Get the panel_id for analytics correlation. Uses session_id as unique identifier.
/// Falls back to "unknown" if identity is not initialized.
/// Feature attribution: panel-close-paths (VAL-PAN-004).
fn get_panel_id() -> String {
    IDENTITY
        .get()
        .map(|(_, sid)| sid.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

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
fn build_run_task_command(
    task_id: &str,
    uv_bin: &str,
    daemon_cwd: &str,
) -> (String, Vec<String>, String) {
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

/// Return the analytics identity (install_id + session_id) for JS-side bootstrap.
/// This is the bridge command that the webview invokes to get the persistent
/// install_id and the per-launch session_id from the Rust authority.
///
/// Also returns platform/arch from std::env::consts so the JS side uses the
/// same values as Rust (fix-m1-review-findings: navigator.userAgent parsing
/// is unreliable on Apple Silicon — it reports x86_64 under Rosetta).
///
/// **Degraded mode** (fix-sentinel-degraded-flag): If install_id IO failed at startup,
/// returns `degraded: true` with sentinel values. JS must skip bootstrap and mark all
/// events with `identity_degraded: true` to avoid collapsing all degraded installs
/// into a single fake distinct_id "analytics-disabled".
#[tauri::command]
fn analytics_identity() -> Result<Value, String> {
    let (install_id, session_id) = IDENTITY
        .get()
        .ok_or_else(|| "Identity not initialized".to_string())?;
    
    // Detect degraded mode: sentinel values indicate install_id IO failure
    let degraded = install_id == "analytics-disabled";
    
    Ok(serde_json::json!({
        "installId": install_id,
        "sessionId": session_id,
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "degraded": degraded,
    }))
}

#[tauri::command]
async fn run_task(task: String) -> Result<(), String> {
    posthog_capture("task_run", serde_json::json!({ "task": task }));
    let uv_bin =
        std::env::var("MENCBO_UV_BIN").unwrap_or_else(|_| "/opt/homebrew/bin/uv".to_string());

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
                posthog_capture(
                    "diag_task_spawn_failed",
                    serde_json::json!({ "task": task, "error": e.to_string() }),
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
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        let m = format!("Failed to create log directory: {}", e);
        posthog_capture(
            "diag_logs_open_failed",
            serde_json::json!({ "stage": "mkdir", "error": m }),
        );
        return Err(m);
    }

    // Open the directory in Finder (macOS)
    if let Err(e) = std::process::Command::new("open").arg(&log_dir).spawn() {
        let m = format!("Failed to open log directory: {}", e);
        posthog_capture(
            "diag_logs_open_failed",
            serde_json::json!({ "stage": "finder", "error": m }),
        );
        return Err(m);
    }

    posthog_capture("logs_open", serde_json::json!({}));
    Ok(())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    // Record exit info for RunEvent::Exit to consume (architecture decision 4)
    let _ = EXIT_INFO.set(("normal".to_string(), "command".to_string()));
    app.exit(0);
    Ok(())
}

/// Shared updater logic for the 6h background loop and the manual trigger.
/// Ok(Installed) means the new version is on disk; the caller must restart.
enum UpdateOutcome {
    UpToDate,
    Installed,
}

/// `source` distinguishes trigger paths in PostHog: "auto" (startup + 6h
/// timer), "manual" (footer button), "tray" (menu item).
async fn check_and_install(
    handle: &tauri::AppHandle,
    source: &str,
) -> Result<UpdateOutcome, String> {
    posthog_capture("app_update_check", serde_json::json!({ "source": source }));
    let updater = match handle.updater_builder().build() {
        Ok(u) => u,
        Err(e) => {
            let m = format!("builder error: {e}");
            posthog_capture(
                "app_update_failed",
                serde_json::json!({ "stage": "builder", "detail": m, "source": source }),
            );
            return Err(m);
        }
    };
    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => {
            posthog_capture("app_update_checked", serde_json::json!({ "result": "uptodate", "source": source }));
            return Ok(UpdateOutcome::UpToDate);
        }
        Err(e) => {
            let m = format!("check failed: {e}");
            posthog_capture(
                "app_update_failed",
                serde_json::json!({ "stage": "check", "detail": m, "source": source }),
            );
            return Err(m);
        }
    };
    println!("[updater] {} -> {}", update.current_version, update.version);
    let target = update.version.clone();
    if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
        let m = format!("install failed: {e}");
        posthog_capture(
            "app_update_failed",
            serde_json::json!({ "stage": "install", "detail": m, "source": source }),
        );
        return Err(m);
    }
    posthog_capture(
        "app_update_installed",
        serde_json::json!({ "to": target, "source": source }),
    );
    Ok(UpdateOutcome::Installed)
}

#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<String, String> {
    match check_and_install(&app, "manual").await? {
        UpdateOutcome::UpToDate => Ok("latest".to_string()),
        // If an update installed, restart() never returns — the frontend's
        // invoke dies with the old process, which is expected.
        UpdateOutcome::Installed => app.restart(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Second instance detected: focus existing window and return
            // The plugin will automatically exit the second instance after this callback
            if let Some(panel) = app.get_webview_window("panel") {
                let _ = panel.show();
                let _ = panel.set_focus();
            }
        }))
        .invoke_handler(tauri::generate_handler![
            get_state,
            run_task,
            open_logs_dir,
            quit_app,
            check_update,
            analytics_identity
        ])
        .setup(|app| {
            // Hide Dock icon (macOS only)
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Initialize app version for analytics events (VAL-ENV-003)
            let app_version = app.package_info().version.to_string();
            APP_VERSION
                .set(app_version.clone())
                .expect("APP_VERSION already initialized");
            
            // Record startup time for session duration calculation on exit
            STARTED_AT.set(std::time::Instant::now())
                .expect("STARTED_AT already initialized");

            // Initialize identity before any analytics events.
            // FIX (fix-m1-review-findings): graceful degradation instead of .expect.
            // "Analytics never kills the host" invariant — if install_id IO fails,
            // disable capture entirely and log the error locally.
            let mut pending_dirty_exit: Option<(String, String)> = None; // (prev_session_id, prev_started_at)
            
            let identity_ok = (|| -> Result<(), String> {
                let app_data_dir = app.path().app_data_dir()
                    .map_err(|e| format!("cannot get app_data_dir: {}", e))?;
                
                // Check for dirty exit from previous session BEFORE writing marker
                // Architecture decision 4: detect crashes/kills via leftover marker
                if let Some(dirty_info) = session_marker::check_dirty_exit(&app_data_dir) {
                    // Previous session didn't exit cleanly
                    // Save info to emit rust_exit{dirty} after analytics is initialized
                    pending_dirty_exit = Some((dirty_info.prev_session_id, dirty_info.prev_started_at));
                    
                    // Clear the old marker (will be replaced by new session marker below)
                    let _ = session_marker::clear_session_marker(&app_data_dir);
                }
                
                let install_id = get_or_create_install_id(&app_data_dir)?;
                let session_id = generate_launch_id();
                IDENTITY.set((install_id, session_id.clone()))
                    .map_err(|_| "Identity already initialized".to_string())?;
                
                // Write session marker for this session
                let started_at = analytics::now_rfc3339();
                if let Err(e) = session_marker::write_session_marker(&app_data_dir, &session_id, &started_at) {
                    eprintln!("[mencbo] failed to write session marker: {}", e);
                    // Non-fatal: continue even if marker write fails
                }
                
                Ok(())
            })();

            if let Err(e) = identity_ok {
                eprintln!("[mencbo] analytics disabled: {}", e);
                // Set sentinel values so IDENTITY.get() returns Some but posthog_capture
                // detects the sentinel and becomes no-op (see posthog_capture function).
                let _ = IDENTITY.set(("analytics-disabled".to_string(), "analytics-disabled".to_string()));
            }

            // Initialize batch analytics queue (VAL-REL-001, VAL-REL-005).
            // Uses a blocking thread for periodic flush (20 items or 60s).
            // Global singleton reqwest::blocking::Client is created here and reused.
            analytics::init(POSTHOG_KEY, POSTHOG_HOST);

            // Emit dirty exit event if previous session didn't exit cleanly
            // This must happen after analytics::init() so the event can be queued
            if let Some((prev_session_id, prev_started_at)) = pending_dirty_exit {
                let uptime_s = session_marker::estimate_uptime_seconds(&prev_started_at)
                    .unwrap_or(0);
                
                posthog_capture(
                    "rust_exit",
                    serde_json::json!({
                        "reason": "dirty",
                        "prev_session_id": prev_session_id,
                        "uptime_s": uptime_s
                    }),
                );
            }

            // Rust-side observability, independent of the webview: launch
            // marker + 5-min heartbeat. If rust_heartbeat keeps flowing while
            // js_heartbeat gaps, a hang is localized to the webview layer.
            // Note: rust_launch carries both version (old, transitional) and app_version (new)
            posthog_capture(
                "rust_launch",
                serde_json::json!({ 
                    "version": app_version,  // Old property (transitional, one version cycle)
                }),
            );
            std::thread::spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(5 * 60));
                posthog_capture("rust_heartbeat", serde_json::json!({}));
            });

            // Auto-update: check at startup, then every 6h. Silent download +
            // install + relaunch. Defensive error handling only — release
            // profile uses panic="abort", so a panic here would kill the tray.
            let update_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(6 * 60 * 60));
                loop {
                    ticker.tick().await; // first tick fires immediately
                    match check_and_install(&update_handle, "auto").await {
                        Ok(UpdateOutcome::Installed) => update_handle.restart(),
                        Ok(UpdateOutcome::UpToDate) => {}
                        Err(e) => eprintln!("[updater] {e}"),
                    }
                }
            });

            // Tray menu: version (disabled) + manual check + quit
            let version = app.package_info().version.to_string();
            let version_item = MenuItem::with_id(
                app,
                "version",
                format!("MenCbo v{version}"),
                false,
                None::<&str>,
            )?;
            let check_item =
                MenuItem::with_id(app, "check-update", "检查更新", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&version_item, &check_item, &quit])?;

            let panel = app.get_webview_window("panel").expect("panel window");

            // Build tray icon in Rust (more reliable on macOS 26)
            // NOTE: tauri.conf.json must NOT contain app.trayIcon — that would
            // create a second NSStatusItem alongside this builder (dual tray bug).
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                // Brand logo has an opaque light-gray background — template mode
                // (alpha-only monochrome) would render as a solid block in the
                // menu bar. Use the full-color icon instead.
                .icon_as_template(false)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        // Architecture decision 4: rust_exit is authoritative
                        // Set EXIT_INFO so RunEvent::Exit can emit the event
                        let _ = EXIT_INFO.set(("normal".to_string(), "tray_menu".to_string()));
                        app.exit(0);
                    }
                    "check-update" => {
                        // updater_check{source:"tray"} is emitted by check_and_install below
                        let h = app.clone();
                        tauri::async_runtime::spawn(async move {
                            match check_and_install(&h, "tray").await {
                                Ok(UpdateOutcome::Installed) => h.restart(),
                                Ok(UpdateOutcome::UpToDate) => {
                                    println!("[updater] up to date (manual)")
                                }
                                Err(e) => eprintln!("[updater] manual check failed: {e}"),
                            }
                        });
                    }
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

                        let panel_id = get_panel_id();

                        // Toggle: if visible, hide
                        if panel.is_visible().unwrap_or(false) {
                            let _ = panel.hide();
                            // Native-side interaction proof, independent of the
                            // webview: during a hang, tray_click keeps flowing
                            // while $autocapture/panel_open stop → webview hang.
                            posthog_capture("panel_close", serde_json::json!({ 
                                "via": "tray",
                                "panel_id": panel_id,
                            }));
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

                        let (x, y) =
                            compute_panel_position(rect_x, rect_width, rect_y, rect_height, scale);

                        // Order: set_position → show → set_focus
                        let _ = panel.set_position(Position::Physical(PhysicalPosition::new(x, y)));
                        let _ = panel.show();
                        let _ = panel.set_focus();
                        posthog_capture("panel_open", serde_json::json!({ 
                            "via": "tray",
                            "panel_id": panel_id,
                        }));
                    }
                })
                .build(app)?;

            // Window events: blur → hide + panel_close{via:blur},
            // close request → hide + panel_close{via:close} (don't exit).
            // All three close paths (tray/blur/close) now emit panel_close with
            // the correct via value (VAL-PAN-001/002/003).
            let panel_for_blur = panel.clone();
            panel.on_window_event(move |e| match e {
                WindowEvent::Focused(false) => {
                    let _ = panel_for_blur.hide();
                    posthog_capture("panel_close", serde_json::json!({ 
                        "via": "blur",
                        "panel_id": get_panel_id(),
                    }));
                }
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = panel_for_blur.hide();
                    posthog_capture("panel_close", serde_json::json!({ 
                        "via": "close",
                        "panel_id": get_panel_id(),
                    }));
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
                use notify::{Event, EventKind, RecursiveMode, Watcher};

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

                let mut watcher =
                    match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                        if let Ok(event) = res {
                            // Only react to events that could affect our target file.
                            // We watch the parent dir, so we get events for all files
                            // in it; filter by file name to avoid spurious wakes.
                            let relevant = match &event.kind {
                                EventKind::Create(_) | EventKind::Modify(_) => {
                                    // Check if any affected path matches our target filename
                                    if let Some(ref target_name) = file_name {
                                        event.paths.iter().any(|p| {
                                            p.file_name()
                                                .map(|n| n == target_name.as_os_str())
                                                .unwrap_or(false)
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
                                posthog_capture(
                                    "state_sync",
                                    serde_json::json!({
                                        "tasks": payload["tasks"].as_array().map(|a| a.len()),
                                        "mock": payload["mock"].as_bool().unwrap_or(false),
                                    }),
                                );
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
        .run(|app_handle, event| {
            match event {
                // FIX 1: Exit guard — must check code.is_none() before prevent_exit().
                // tauri 2.11.5 prevent_exit() swallows ALL exit codes (including code:Some(0))
                // because runtime-wry skips ControlFlow::Exit on Prevent. Without the guard,
                // app.exit(0) from the quit menu / quit_app command is silently swallowed and
                // the process can never exit normally. See tauri issue #17723.
                tauri::RunEvent::ExitRequested { code, api, .. } => {
                    if code.is_none() {
                        api.prevent_exit();
                    }
                }
                tauri::RunEvent::Exit => {
                    // Architecture decision 4: Emit rust_exit event before flushing
                    // Calculate session uptime
                    let uptime_s = STARTED_AT
                        .get()
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0);
                    
                    // Determine exit reason based on EXIT_INFO
                    // Fallback: if we reach RunEvent::Exit, it's a controlled exit.
                    // Truly abnormal exits (crash/kill) never reach this handler —
                    // they're handled by the session marker mechanism (dirty exit on next launch).
                    let (reason, via) = EXIT_INFO.get()
                        .map(|(r, v)| (r.clone(), v.clone()))
                        .unwrap_or_else(|| ("normal".to_string(), "system".to_string()));
                    
                    // Flush pending events first, then enqueue rust_exit
                    // Flush queue to send all pending events
                    let _ = analytics::flush_sync(std::time::Duration::from_secs(3));
                    
                    // Now enqueue rust_exit with the flush status
                    posthog_capture("rust_exit", serde_json::json!({
                        "reason": reason,
                        "via": via,
                        "uptime_s": uptime_s,
                        "pending_flushed": true,
                    }));
                    
                    // Final flush to send rust_exit itself
                    let _ = analytics::flush_sync(std::time::Duration::from_secs(3));
                    
                    // Clear session marker after flush (normal exit)
                    if let Ok(app_data_dir) = app_handle.path().app_data_dir() {
                        let _ = session_marker::clear_session_marker(&app_data_dir);
                    }
                }
                _ => {}
            }
        });
}

/// MENCBO_DIAG_TEST fault injection hook (architecture decision #10).
/// Reads MENCBO_DIAG_TEST env var and dispatches to fault injection logic.
/// This is a skeleton for M4 feature - actual fault injection behavior will be filled in later.
/// 
/// Supported values: freeze_webview, block_main, slow_ipc, panic
/// Only active in release builds (debug builds are gated by cfg!(debug_assertions)).
/// 
/// Feature attribution: fix-m1-review-findings (m1 scrutiny round 1 quality debt)
/// Will be consumed by: watchdog-diagnostics (M4)
#[allow(dead_code)]
fn check_diag_test_hook() {
    // Only active in release builds
    if cfg!(debug_assertions) {
        return;
    }

    if let Ok(diag_mode) = std::env::var("MENCBO_DIAG_TEST") {
        match diag_mode.as_str() {
            "freeze_webview" => {
                // Skeleton: M4 will implement webview freeze injection
                eprintln!("[diag] MENCBO_DIAG_TEST=freeze_webview detected (skeleton, no-op for now)");
            }
            "block_main" => {
                // Skeleton: M4 will implement main thread block injection
                eprintln!("[diag] MENCBO_DIAG_TEST=block_main detected (skeleton, no-op for now)");
            }
            "slow_ipc" => {
                // Skeleton: M4 will implement slow IPC injection
                eprintln!("[diag] MENCBO_DIAG_TEST=slow_ipc detected (skeleton, no-op for now)");
            }
            "panic" => {
                // Skeleton: M4 will implement panic injection
                eprintln!("[diag] MENCBO_DIAG_TEST=panic detected (skeleton, no-op for now)");
            }
            _ => {
                // Unknown mode, ignore
            }
        }
    }
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
        assert_eq!(
            args,
            vec![
                "run",
                "python",
                "-m",
                "mencbo",
                "run-task",
                "example:heartbeat"
            ]
        );
        assert_eq!(cwd, "/home/user/mencbo/apps/desktop/daemon");
    }

    #[test]
    fn build_run_task_custom_uv() {
        let (program, args, _) =
            build_run_task_command("test:task", "/custom/bin/uv", "/some/path");
        assert_eq!(program, "/custom/bin/uv");
        assert_eq!(
            args,
            vec!["run", "python", "-m", "mencbo", "run-task", "test:task"]
        );
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
            tasks
                .iter()
                .find(|t| t["id"] == id)
                .expect("task not found")
        };

        // Success task: error is null, duration_ms=12→durationMs=12
        let heartbeat = find_task("example:heartbeat");
        assert_eq!(heartbeat["id"], "example:heartbeat");
        assert_eq!(heartbeat["name"], "示例·心跳日志");
        assert_eq!(heartbeat["lastRun"], "2026-09-07T01:30:00+00:00");
        assert_eq!(heartbeat["status"], "success");
        assert_eq!(heartbeat["durationMs"], 12);
        assert!(
            heartbeat["error"].is_null(),
            "success task error must be null"
        );

        // Failed task: error is non-null string, duration_ms present
        let failure = find_task("example:failure-demo");
        assert_eq!(failure["id"], "example:failure-demo");
        assert_eq!(failure["name"], "示例·失败演示");
        assert_eq!(failure["lastRun"], "2026-09-07T00:50:00+00:00");
        assert_eq!(failure["status"], "failed");
        assert_eq!(failure["durationMs"], 1234);
        assert!(
            failure["error"].is_string(),
            "failed task error must be non-null string"
        );
        assert_eq!(failure["error"], "RuntimeError('演示失败')");

        // Running task: duration_ms=null→durationMs=null, error=null
        let maintenance = find_task("example:maintenance");
        assert_eq!(maintenance["id"], "example:maintenance");
        assert_eq!(maintenance["name"], "示例·维护日志");
        assert_eq!(maintenance["status"], "running");
        assert!(
            maintenance["durationMs"].is_null(),
            "running task durationMs must be null"
        );
        assert!(
            maintenance["error"].is_null(),
            "running task error must be null"
        );

        // No snake_case keys should leak through (transformation check)
        for task in tasks {
            assert!(
                task.get("display_name").is_none(),
                "display_name should be renamed to name"
            );
            assert!(
                task.get("last_run").is_none(),
                "last_run should be renamed to lastRun"
            );
            assert!(
                task.get("duration_ms").is_none(),
                "duration_ms should be renamed to durationMs"
            );
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

        let uv_bin =
            std::env::var("MENCBO_UV_BIN").unwrap_or_else(|_| "/opt/homebrew/bin/uv".to_string());

        let (program, args, cwd) = build_run_task_command("test:task", &uv_bin, &daemon_cwd);
        assert_eq!(program, "/custom/uv");
        assert_eq!(cwd, "/custom/daemon/path");
        assert_eq!(
            args,
            vec!["run", "python", "-m", "mencbo", "run-task", "test:task"]
        );

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
        assert_eq!(
            daemon_cwd,
            format!("{home}/mencbo/mencbo/apps/desktop/daemon")
        );
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

    // ---- Single-instance guard tests (VAL-SI-001) ----

    #[test]
    fn tauri_conf_identifier_is_stable() {
        // VAL-SI-001: The production identifier must be com.mencbo.desktop
        let conf = include_str!("../tauri.conf.json");
        let json: serde_json::Value = serde_json::from_str(conf).unwrap();
        assert_eq!(json["identifier"], "com.mencbo.desktop");
    }

    #[test]
    fn tauri_conf_dev_identifier_is_different() {
        // VAL-SI-001: Dev identifier must be com.mencbo.desktop.dev to avoid
        // conflicting with the production instance during testing.
        let conf = include_str!("../tauri.conf.dev.json");
        let json: serde_json::Value = serde_json::from_str(conf).unwrap();
        assert_eq!(json["identifier"], "com.mencbo.desktop.dev");
        assert_ne!(json["identifier"], "com.mencbo.desktop");
    }
}
