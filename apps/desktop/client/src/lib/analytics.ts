import posthog from "posthog-js";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";

// PostHog US project "MenCbo Desktop" — capture-only key, injected at build time via VITE_POSTHOG_KEY env var.
// If not set (local dev without explicit injection), falls back to "NO_KEY" sentinel and all captures become no-op.
const POSTHOG_KEY = import.meta.env.VITE_POSTHOG_KEY || "NO_KEY";
const POSTHOG_HOST = "https://us.i.posthog.com";

let launchedAt = 0;
let analyticsReady = false;

/**
 * Initialize PostHog analytics with unified identity from Rust.
 *
 * Environment gating: import.meta.env.DEV returns early (VAL-ENV-001).
 * This is the JS-side equivalent of Rust's cfg!(debug_assertions) gate.
 * Debug builds never initialize PostHog or send any events.
 *
 * If the identity invoke or posthog.init with bootstrap fails, we:
 * 1. Fall back to a basic posthog.init (no bootstrap, persistence:memory)
 * 2. Capture a `diag_identity_failed` event with the failure reason
 * 3. Still register error listeners and heartbeat (failure must be observable)
 *
 * This ensures the failure path is always visible in PostHog — an architectural
 * invariant (see p0-identity-unification round 3 diagnosis).
 */
export async function initAnalytics() {
  // JS-side debug gate (VAL-ENV-001: dev 构建零上报)
  if (import.meta.env.DEV) {
    return;
  }

  if (!POSTHOG_KEY || POSTHOG_KEY === "NO_KEY") {
    return;
  }

  let identityOk = false;
  let installId = "";
  let sessionId = "";
  let failReason = "";

  // Step 1: try to fetch identity from Rust (with 3s timeout)
  try {
    const identityPromise = invoke<{ installId: string; sessionId: string }>(
      "analytics_identity",
    );
    const timeoutPromise = new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error("invoke timeout after 3s")), 3000)
    );
    const identity = await Promise.race([identityPromise, timeoutPromise]);
    installId = identity.installId;
    sessionId = identity.sessionId;
    identityOk = true;
  } catch (e) {
    failReason = `invoke_failed: ${String(e).slice(0, 200)}`;
  }

  // Step 2: init posthog — with bootstrap if identity succeeded
  let initOk = false;
  if (identityOk) {
    try {
      posthog.init(POSTHOG_KEY, {
        api_host: POSTHOG_HOST,
        bootstrap: {
          distinctID: installId,
          isIdentifiedID: true,
        },
        autocapture: true,
        capture_pageview: false,
        persistence: "memory",
      });
      initOk = true;
    } catch (e) {
      failReason = `init_bootstrap_failed: ${String(e).slice(0, 200)}`;
    }
  }

  // Step 3: fallback init without bootstrap if anything failed
  if (!initOk) {
    try {
      posthog.init(POSTHOG_KEY, {
        api_host: POSTHOG_HOST,
        autocapture: true,
        capture_pageview: false,
        persistence: "memory",
      });
      initOk = true;
    } catch (e) {
      // Total failure — can't init posthog at all.
      // Still register listeners below so at least the architecture is intact
      // for when/if posthog becomes available later.
      failReason = `init_fallback_also_failed: ${String(e).slice(0, 200)}`;
    }
  }

  // Step 4: register baseline properties and capture diagnostic + launch
  if (initOk) {
    if (identityOk) {
      // Get app version from Tauri API (VAL-ENV-003)
      const appVersion = await getVersion().catch(() => "unknown");
      
      posthog.register({
        session_id: sessionId,
        source: "webview",
        // Environment properties (VAL-ENV-002/003)
        app_version: appVersion,
        environment: "production", // JS only runs in release builds after DEV gate
        platform: navigator.platform || "unknown",
        arch: navigator.userAgent.includes("arm64") || navigator.userAgent.includes("aarch64") 
          ? "aarch64" 
          : "x86_64",
      });
    }

    analyticsReady = true;
    launchedAt = Date.now();

    // If identity failed, capture the diagnostic event first
    if (!identityOk || failReason) {
      capture("diag_identity_failed", { reason: failReason });
    }

    // js_launch — now that posthog is initialized
    capture("js_launch");
  }

  // Step 5: ALWAYS register error listeners and heartbeat, regardless of
  // whether identity/init succeeded. This is the architectural invariant:
  // the failure path must be observable.
  window.addEventListener("error", (e) =>
    capture("js_error", {
      message: String(e.message).slice(0, 300),
      line: e.lineno,
    }),
  );
  window.addEventListener("unhandledrejection", (e) =>
    capture("js_unhandled_rejection", { reason: String(e.reason).slice(0, 300) }),
  );

  // JS heartbeat (5 min). Compare with rust_heartbeat in PostHog: if rust
  // keeps flowing while js gaps, the hang is in the webview layer.
  window.setInterval(() => {
    if (analyticsReady) {
      capture("js_heartbeat", {
        uptime_sec: Math.round((Date.now() - launchedAt) / 1000),
      });
    }
  }, 5 * 60 * 1000);
}

export function capture(event: string, props?: Record<string, unknown>) {
  try {
    posthog.capture(event, { app: "mencbo-desktop", ...props });
  } catch {
    /* never break the app for analytics */
  }
}
