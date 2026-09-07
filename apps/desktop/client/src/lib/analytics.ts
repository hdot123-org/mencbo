import posthog from "posthog-js";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";

// PostHog US project "MenCbo Desktop" — capture-only key, injected at build time via VITE_POSTHOG_KEY env var.
// If not set (local dev without explicit injection), falls back to "NO_KEY" sentinel and all captures become no-op.
const POSTHOG_KEY = import.meta.env.VITE_POSTHOG_KEY || "NO_KEY";
const POSTHOG_HOST = "https://us.i.posthog.com";

let launchedAt = 0;
let analyticsReady = false;
let identityDegraded = false; // fix-sentinel-degraded-flag: true when install_id IO failed

// Event buffer: events captured before identity is ready are queued here
// and flushed in order once register() completes (fixes session_id race).
const eventBuffer: Array<{ event: string; props?: Record<string, unknown> }> = [];

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
  let platform = navigator.platform || "unknown";
  let arch = navigator.userAgent?.includes("arm64") || navigator.userAgent?.includes("aarch64")
    ? "aarch64"
    : "x86_64";

  // Step 1: try to fetch identity from Rust (with 3s timeout)
  try {
    const identityPromise = invoke<{ installId: string; sessionId: string; platform: string; arch: string; degraded?: boolean }>(
      "analytics_identity",
    );
    const timeoutPromise = new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error("invoke timeout after 3s")), 3000)
    );
    const identity = await Promise.race([identityPromise, timeoutPromise]);
    installId = identity.installId;
    sessionId = identity.sessionId;
    identityOk = true;
    // fix-sentinel-degraded-flag: detect degraded mode from Rust
    identityDegraded = identity.degraded === true;
    // Store platform/arch from Rust bridge (Fix 1: navigator.userAgent is unreliable on Apple Silicon)
    platform = identity.platform;
    arch = identity.arch;
  } catch (e) {
    failReason = `invoke_failed: ${String(e).slice(0, 200)}`;
  }

  // Step 2: init posthog — with bootstrap if identity succeeded AND not degraded
  let initOk = false;
  if (identityOk && !identityDegraded) {
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
  } else if (identityOk && identityDegraded) {
    // fix-sentinel-degraded-flag: degraded mode detected, skip bootstrap to avoid
    // collapsing all degraded installs into fake distinct_id "analytics-disabled"
    try {
      posthog.init(POSTHOG_KEY, {
        api_host: POSTHOG_HOST,
        autocapture: true,
        capture_pageview: false,
        persistence: "memory",
      });
      initOk = true;
    } catch (e) {
      failReason = `init_degraded_failed: ${String(e).slice(0, 200)}`;
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
    // Get app version from Tauri API (VAL-ENV-003)
    const appVersion = await getVersion().catch(() => "unknown");
    
    // Register baseline properties for ALL paths (Fix 2: fallback path must have baselines)
    // fix-sentinel-degraded-flag: include identity_degraded flag when install_id IO failed
    posthog.register({
      session_id: identityOk ? sessionId : "missing",
      source: "webview",
      // Environment properties (VAL-ENV-002/003)
      app_version: appVersion,
      environment: "production", // JS only runs in release builds after DEV gate
      platform: platform,
      arch: arch,
      // fix-sentinel-degraded-flag: mark all events from degraded installs
      identity_degraded: identityDegraded,
    });

    analyticsReady = true;
    launchedAt = Date.now();

    // Flush any events that were buffered before identity was ready
    // This fixes the race condition where state_load fires before session_id is set
    flushBufferedEvents();

    // fix-sentinel-degraded-flag: in degraded mode, capture diagnostic event
    // to make the degradation observable in PostHog
    if (identityDegraded) {
      capture("diag_identity_failed", { reason: "install_id_io_failed, identity degraded" });
    }

    // If identity failed (not degraded, but invoke/init failed), capture the diagnostic event
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
    if (!analyticsReady) {
      // Buffer event until identity is ready (fixes session_id race)
      eventBuffer.push({ event, props });
      return;
    }
    posthog.capture(event, { app: "mencbo-desktop", ...props });
  } catch {
    /* never break the app for analytics */
  }
}

function flushBufferedEvents() {
  // Flush all buffered events in order after register() is called
  const buffered = eventBuffer.splice(0, eventBuffer.length);
  for (const { event, props } of buffered) {
    try {
      posthog.capture(event, { app: "mencbo-desktop", ...props });
    } catch {
      /* never break the app for analytics */
    }
  }
}
