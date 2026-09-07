/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import posthog from "posthog-js";

// Mock VITE_POSTHOG_KEY environment variable before importing analytics
vi.stubEnv("VITE_POSTHOG_KEY", "test-key-12345");
// Mock DEV to false so tests can exercise the full analytics flow
// (in real dev environment, DEV=true and initAnalytics returns early)
vi.stubEnv("DEV", false);

// Mock posthog-js
vi.mock("posthog-js", () => ({
  default: {
    init: vi.fn(),
    capture: vi.fn(),
    register: vi.fn(),
  },
}));

// Mock @tauri-apps/api/core
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

// Mock @tauri-apps/api/app - must return a Promise for .catch() chain
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(() => Promise.resolve("0.2.4")),
}));

describe("analytics identity injection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset modules to get fresh imports
    vi.resetModules();
    // Re-stub env after resetModules
    vi.stubEnv("VITE_POSTHOG_KEY", "test-key-12345");
    vi.stubEnv("DEV", false);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("applies bootstrap distinctID from identity command", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockResolvedValue({
      installId: "desktop-test-uuid-1234",
      sessionId: "session-5678",
      platform: "darwin",
      arch: "aarch64",
    });

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    expect(posthog.init).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({
        bootstrap: {
          distinctID: "desktop-test-uuid-1234",
          isIdentifiedID: true,
        },
      })
    );
  });

  it("js_launch waits for identity", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockResolvedValue({
      installId: "desktop-test-uuid-1234",
      sessionId: "session-5678",
    });

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    // js_launch should be called after identity is ready
    expect(posthog.capture).toHaveBeenCalledWith(
      "js_launch",
      expect.objectContaining({ app: "mencbo-desktop" })
    );
  });

  it("register includes session_id and source", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockResolvedValue({
      installId: "desktop-test-uuid-1234",
      sessionId: "session-5678",
      platform: "darwin",
      arch: "aarch64",
    });

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    expect(posthog.register).toHaveBeenCalledWith(
      expect.objectContaining({
        session_id: "session-5678",
        source: "webview",
        environment: "production",
        app_version: expect.any(String),
        platform: "darwin",
        arch: "aarch64",
      })
    );
  });

  it("no random anonymous distinct_id", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockResolvedValue({
      installId: "desktop-test-uuid-1234",
      sessionId: "session-5678",
    });

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    // Verify that bootstrap.distinctID is the installId, not a random UUID
    const initCall = vi.mocked(posthog.init).mock.calls[0];
    const options = initCall[1] as any;
    expect(options.bootstrap.distinctID).toBe("desktop-test-uuid-1234");
    expect(options.bootstrap.isIdentifiedID).toBe(true);
  });

  it("handles identity fetch failure gracefully with fallback", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockRejectedValue(new Error("Identity not available"));

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    // NEW BEHAVIOR: When identity fails, we fall back to basic init (no bootstrap)
    // This ensures the failure path is observable in PostHog
    expect(posthog.init).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({
        api_host: "https://us.i.posthog.com",
        autocapture: true,
        capture_pageview: false,
        persistence: "memory",
      })
    );

    // Verify the fallback init was called WITHOUT bootstrap
    const initCalls = vi.mocked(posthog.init).mock.calls;
    expect(initCalls.length).toBe(1);
    const initOptions = initCalls[0][1] as any;
    expect(initOptions.bootstrap).toBeUndefined();

    // Verify diagnostic event was captured with the failure reason
    expect(posthog.capture).toHaveBeenCalledWith(
      "diag_identity_failed",
      expect.objectContaining({
        app: "mencbo-desktop",
        reason: expect.stringContaining("invoke_failed"),
      })
    );

    // js_launch should still be captured (analytics is observable even on failure)
    expect(posthog.capture).toHaveBeenCalledWith(
      "js_launch",
      expect.objectContaining({ app: "mencbo-desktop" })
    );

    // NEW BEHAVIOR: register should be called even when identity fails
    // This ensures baseline properties are present on all events (Fix 2)
    expect(posthog.register).toHaveBeenCalledWith(
      expect.objectContaining({
        session_id: "missing",
        source: "webview",
        environment: "production",
        app_version: expect.any(String),
        platform: expect.any(String),
        arch: expect.any(String),
      })
    );
  });

  it("fix-sentinel-degraded-flag: degraded mode skips bootstrap and marks identity_degraded", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    // Simulate degraded mode: Rust returns sentinel values with degraded=true
    mockInvoke.mockResolvedValue({
      installId: "analytics-disabled",
      sessionId: "analytics-disabled",
      platform: "darwin",
      arch: "aarch64",
      degraded: true,
    });

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    // Verify posthog.init was called WITHOUT bootstrap (degraded mode skips it)
    const initCalls = vi.mocked(posthog.init).mock.calls;
    expect(initCalls.length).toBe(1);
    const initOptions = initCalls[0][1] as any;
    expect(initOptions.bootstrap).toBeUndefined();

    // Verify register includes identity_degraded: true
    expect(posthog.register).toHaveBeenCalledWith(
      expect.objectContaining({
        identity_degraded: true,
        session_id: "analytics-disabled",
        source: "webview",
      })
    );

    // Verify diagnostic event captured for degraded mode
    expect(posthog.capture).toHaveBeenCalledWith(
      "diag_identity_failed",
      expect.objectContaining({
        app: "mencbo-desktop",
        reason: expect.stringContaining("identity degraded"),
      })
    );

    // js_launch should still be captured (analytics observable even when degraded)
    expect(posthog.capture).toHaveBeenCalledWith(
      "js_launch",
      expect.objectContaining({ app: "mencbo-desktop" })
    );
  });

  it("fix-sentinel-degraded-flag: normal mode uses bootstrap and sets identity_degraded:false", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    // Normal mode: valid identity with degraded=false
    mockInvoke.mockResolvedValue({
      installId: "desktop-normal-uuid-1234",
      sessionId: "session-normal-5678",
      platform: "darwin",
      arch: "aarch64",
      degraded: false,
    });

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    // Verify posthog.init was called WITH bootstrap (normal mode)
    expect(posthog.init).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({
        bootstrap: {
          distinctID: "desktop-normal-uuid-1234",
          isIdentifiedID: true,
        },
      })
    );

    // Verify register includes identity_degraded: false
    expect(posthog.register).toHaveBeenCalledWith(
      expect.objectContaining({
        identity_degraded: false,
        session_id: "session-normal-5678",
      })
    );

    // Verify NO diagnostic event for normal mode
    const captureCalls = vi.mocked(posthog.capture).mock.calls;
    const diagCalls = captureCalls.filter(call => call[0] === "diag_identity_failed");
    expect(diagCalls.length).toBe(0);
  });

  it("buffers events captured before identity is ready (race condition fix)", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    
    // Simulate delayed identity resolution (e.g., 50ms)
    let resolveIdentity: (value: any) => void;
    const identityPromise = new Promise((resolve) => {
      resolveIdentity = resolve;
    });
    mockInvoke.mockReturnValue(identityPromise as any);

    // Import and start init (but don't await yet)
    const { initAnalytics, capture } = await import("../analytics");
    const initPromise = initAnalytics();

    // Capture an event BEFORE identity is ready (simulates App.tsx state_load race)
    capture("state_load", { tasks: 5, duration_ms: 10 });
    capture("state_sync", { tasks: 5 });

    // Verify no events were sent yet (buffered)
    expect(posthog.capture).not.toHaveBeenCalledWith(
      "state_load",
      expect.anything()
    );

    // Now resolve identity
    resolveIdentity!({
      installId: "desktop-test-uuid-1234",
      sessionId: "session-5678",
      platform: "darwin",
      arch: "aarch64",
    });

    // Wait for init to complete
    await initPromise;

    // Verify buffered events were flushed IN ORDER after identity was ready
    const captureCalls = vi.mocked(posthog.capture).mock.calls;
    const eventNames = captureCalls.map((call) => call[0]);
    
    // Buffered events should be flushed first in original order
    const stateLoadIndex = eventNames.indexOf("state_load");
    const stateSyncIndex = eventNames.indexOf("state_sync");
    const jsLaunchIndex = eventNames.indexOf("js_launch");
    
    expect(stateLoadIndex).toBeGreaterThanOrEqual(0);
    expect(stateSyncIndex).toBeGreaterThan(stateLoadIndex);
    
    // js_launch comes after buffered events (it's captured after flush)
    expect(jsLaunchIndex).toBeGreaterThan(stateSyncIndex);
    
    // Verify state_load has correct properties
    const stateLoadCall = captureCalls.find((call) => call[0] === "state_load");
    expect(stateLoadCall).toBeDefined();
    expect(stateLoadCall![1]).toEqual(
      expect.objectContaining({
        tasks: 5,
        duration_ms: 10,
        app: "mencbo-desktop",
      })
    );
  });
});
