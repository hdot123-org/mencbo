/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import posthog from "posthog-js";

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
  });

  it("applies bootstrap distinctID from identity command", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockResolvedValue({
      installId: "desktop-test-uuid-1234",
      sessionId: "session-5678",
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
    });

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    expect(posthog.register).toHaveBeenCalledWith({
      session_id: "session-5678",
      source: "webview",
    });
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

  it("handles identity fetch failure gracefully", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const mockInvoke = vi.mocked(invoke);
    mockInvoke.mockRejectedValue(new Error("Identity not available"));

    const { initAnalytics } = await import("../analytics");
    await initAnalytics();

    // Should not initialize posthog if identity fetch fails
    expect(posthog.init).not.toHaveBeenCalled();
    expect(posthog.capture).not.toHaveBeenCalled();
  });
});
