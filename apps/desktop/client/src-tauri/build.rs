/// Resolve the build identifier (git commit SHA) for analytics attribution.
///
/// FIX (fix-build-attribution): Diag events carried only `app_version`, so two
/// builds sharing a version (e.g. pre-fix and post-fix 0.2.4 main builds) were
/// indistinguishable in PostHog. Fix-verification probes then misattributed
/// events from stale builds to the fix commit.
///
/// Resolution order:
/// 1. `MENCBO_BUILD_SHA` env var (CI reproducibility / explicit override)
/// 2. `git rev-parse --short HEAD` at compile time
/// 3. "unknown" sentinel (tarball builds without .git)
fn resolve_build_sha() -> String {
    if let Ok(from_env) = std::env::var("MENCBO_BUILD_SHA") {
        if !from_env.is_empty() {
            return from_env;
        }
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    // Inject PostHog key from environment at build time.
    // Falls back to a sentinel value if not set, which makes all captures no-op.
    let posthog_key = std::env::var("POSTHOG_KEY").unwrap_or_else(|_| "NO_KEY".to_string());
    println!("cargo:rustc-env=POSTHOG_KEY={}", posthog_key);
    println!("cargo:rerun-if-env-changed=POSTHOG_KEY");

    // Inject build SHA for analytics attribution (fix-build-attribution)
    println!("cargo:rustc-env=MENCBO_BUILD_SHA={}", resolve_build_sha());
    println!("cargo:rerun-if-env-changed=MENCBO_BUILD_SHA");

    tauri_build::build()
}
