/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
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

describe("analytics identity injection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset modules to get fresh imports
    vi.resetModules();
    // Re-stub env after resetModules
    vi.stubEnv("VITE_POSTHOG_KEY", "test-key-12345");
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
});
