fn main() {
    // Inject PostHog key from environment at build time.
    // Falls back to a sentinel value if not set, which makes all captures no-op.
    let posthog_key = std::env::var("POSTHOG_KEY").unwrap_or_else(|_| "NO_KEY".to_string());
    println!("cargo:rustc-env=POSTHOG_KEY={}", posthog_key);
    println!("cargo:rerun-if-env-changed=POSTHOG_KEY");
    
    tauri_build::build()
}
