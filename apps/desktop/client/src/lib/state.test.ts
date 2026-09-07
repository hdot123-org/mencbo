import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

// Mock @tauri-apps/api modules
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

// Mock env.ts to control inTauri
vi.mock("./env", () => ({
  get inTauri() {
    return (globalThis as any).__TEST_IN_TAURI ?? false;
  },
}));

describe("loadState (browser mode)", () => {
  beforeEach(() => {
    (globalThis as any).__TEST_IN_TAURI = false;
    vi.resetModules();
  });

  afterEach(() => {
    delete (globalThis as any).__TEST_IN_TAURI;
    vi.unstubAllGlobals();
  });

  it("returns mock state with default scenario", async () => {
    const { loadState } = await import("./state");
    const result = await loadState();
    expect(result.mock).toBe(true);
    expect(result.tasks).toHaveLength(3);
    expect(result.tasks.map((t: any) => t.id).sort()).toEqual([
      "example:failure-demo",
      "example:heartbeat",
      "example:maintenance",
    ]);
  });

  it("returns mock state with ok scenario (2 tasks, no failure)", async () => {
    // Stub globalThis.location so getScenario() sees the query param
    vi.stubGlobal("location", { search: "?scenario=ok" });
    // Also need to stub window since getScenario checks typeof window
    vi.stubGlobal("window", { location: { search: "?scenario=ok" } });
    const { loadState } = await import("./state");
    const result = await loadState();
    expect(result.mock).toBe(true);
    expect(result.tasks).toHaveLength(2);
    expect(result.tasks.every((t: any) => t.status !== "failed")).toBe(true);
  });

  it("returns mock state with empty scenario (0 tasks)", async () => {
    vi.stubGlobal("location", { search: "?scenario=empty" });
    vi.stubGlobal("window", { location: { search: "?scenario=empty" } });
    const { loadState } = await import("./state");
    const result = await loadState();
    expect(result.mock).toBe(true);
    expect(result.tasks).toHaveLength(0);
  });
});

describe("loadState (Tauri mode)", () => {
  beforeEach(() => {
    (globalThis as any).__TEST_IN_TAURI = true;
    vi.resetModules();
  });

  afterEach(() => {
    delete (globalThis as any).__TEST_IN_TAURI;
    vi.restoreAllMocks();
  });

  it("returns real state from Rust when not mock", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const realState = {
      mock: false,
      health: "ok",
      tasks: [
        {
          id: "test:task",
          name: "Test Task",
          status: "success",
          lastRun: new Date().toISOString(),
          durationMs: 123,
        },
      ],
    };
    vi.mocked(invoke).mockResolvedValue(realState);

    const { loadState } = await import("./state");
    const result = await loadState();
    expect(result).toEqual(realState);
  });

  it("falls back to built-in mock when Rust returns {mock: true, tasks: []} (VAL-CROSS-005)", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValue({ mock: true, tasks: [] });

    const { loadState } = await import("./state");
    const result = await loadState();
    // Should fall back to built-in mock fixture (3 demo tasks)
    expect(result.mock).toBe(true);
    expect(result.tasks).toHaveLength(3);
    const ids = result.tasks.map((t) => t.id).sort();
    expect(ids).toEqual([
      "example:failure-demo",
      "example:heartbeat",
      "example:maintenance",
    ]);
  });

  it("falls back to built-in mock when invoke throws", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockRejectedValue(new Error("Tauri not available"));

    const { loadState } = await import("./state");
    const result = await loadState();
    expect(result.mock).toBe(true);
    expect(result.tasks).toHaveLength(3);
  });
});

describe("triggerTask", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    delete (globalThis as any).__TEST_IN_TAURI;
    vi.restoreAllMocks();
  });

  it("calls invoke in Tauri mode", async () => {
    (globalThis as any).__TEST_IN_TAURI = true;
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValue(undefined);

    const { triggerTask } = await import("./state");
    await triggerTask("test:task");

    expect(invoke).toHaveBeenCalledWith("run_task", { task: "test:task" });
  });

  it("logs to console in browser mode without throwing", async () => {
    (globalThis as any).__TEST_IN_TAURI = false;
    const consoleSpy = vi.spyOn(console, "log").mockImplementation(() => {});

    const { triggerTask } = await import("./state");
    await expect(triggerTask("test:task")).resolves.toBeUndefined();
    expect(consoleSpy).toHaveBeenCalledWith("[mock] trigger task:", "test:task");
  });
});

describe("subscribeState", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    delete (globalThis as any).__TEST_IN_TAURI;
    vi.restoreAllMocks();
  });

  it("no-op in browser mode", async () => {
    (globalThis as any).__TEST_IN_TAURI = false;
    vi.resetModules();
    const { subscribeState } = await import("./state");
    const cb = vi.fn();
    const unlisten = subscribeState(cb);
    expect(typeof unlisten).toBe("function");
    expect(cb).not.toHaveBeenCalled();
  });

  it("falls back to MOCK_STATE when event payload is {mock: true, tasks: []}", async () => {
    (globalThis as any).__TEST_IN_TAURI = true;

    // Mock the listen function to simulate an event
    const mockListen = vi.fn((_event: string, callback: any) => {
      // Simulate Rust emitting a mock payload
      setTimeout(() => {
        callback({ payload: { mock: true, tasks: [] } });
      }, 10);
      return Promise.resolve(() => {});
    });

    vi.doMock("@tauri-apps/api/event", () => ({
      listen: mockListen,
    }));

    const { subscribeState } = await import("./state");
    const cb = vi.fn();
    subscribeState(cb);

    // Wait for the async event to fire
    await new Promise((resolve) => setTimeout(resolve, 50));

    // Should have called the callback with MOCK_STATE (3 tasks), not the empty payload
    expect(cb).toHaveBeenCalledTimes(1);
    const receivedState = cb.mock.calls[0][0];
    expect(receivedState.mock).toBe(true);
    expect(receivedState.tasks).toHaveLength(3);
    const ids = receivedState.tasks.map((t: any) => t.id).sort();
    expect(ids).toEqual([
      "example:failure-demo",
      "example:heartbeat",
      "example:maintenance",
    ]);
  });

  it("passes through non-mock payloads unchanged", async () => {
    (globalThis as any).__TEST_IN_TAURI = true;

    const realState = {
      mock: false,
      health: "ok",
      tasks: [
        {
          id: "real:task",
          name: "Real Task",
          status: "success",
          lastRun: new Date().toISOString(),
          durationMs: 100,
        },
      ],
    };

    const mockListen = vi.fn((_event: string, callback: any) => {
      setTimeout(() => {
        callback({ payload: realState });
      }, 10);
      return Promise.resolve(() => {});
    });

    vi.doMock("@tauri-apps/api/event", () => ({
      listen: mockListen,
    }));

    const { subscribeState } = await import("./state");
    const cb = vi.fn();
    subscribeState(cb);

    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith(realState);
  });
});
