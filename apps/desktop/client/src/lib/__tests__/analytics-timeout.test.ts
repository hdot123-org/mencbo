import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

// Mock the invoke function from @tauri-apps/api/core
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

// Import after mocking — invokeWithTimeout uses invoke from @tauri-apps/api/core internally
import { invokeWithTimeout } from "../analytics";

describe("invokeWithTimeout", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("should resolve when invoke completes before timeout", async () => {
    const mockResult = { data: "test" };
    vi.mocked(invoke).mockResolvedValue(mockResult);

    const promise = invokeWithTimeout("test_command", {}, 3000);
    const result = await promise;

    expect(result).toEqual(mockResult);
    expect(invoke).toHaveBeenCalledWith("test_command", {});
  });

  it("should reject with timeout error when invoke exceeds timeout", async () => {
    // Mock invoke to resolve very late (after timeout), using setTimeout so fake timers control it
    vi.mocked(invoke).mockImplementation(
      () => new Promise((resolve) => setTimeout(() => resolve(null), 60_000))
    );

    const promise = invokeWithTimeout("slow_command", {}, 3000);

    // Attach rejection handler BEFORE advancing timers to prevent unhandled rejection
    const caughtError: Promise<unknown> = promise.catch((e) => e);

    // Advance past the timeout (but before the mock resolves)
    await vi.advanceTimersByTimeAsync(3001);

    // Should reject with timeout error
    const error = await caughtError as Error;
    expect(error.message).toBe("IPC timeout: slow_command exceeded 3000ms");

    // Advance past mock resolve to clean up
    await vi.advanceTimersByTimeAsync(60_000);
  });

  it("should use default timeout of 3000ms when not specified", async () => {
    vi.mocked(invoke).mockImplementation(
      () => new Promise((resolve) => setTimeout(() => resolve(null), 60_000))
    );

    const promise = invokeWithTimeout("test_command");

    // Attach rejection handler BEFORE advancing timers to prevent unhandled rejection
    const caughtError: Promise<unknown> = promise.catch((e) => e);

    // Should timeout after default 3000ms
    await vi.advanceTimersByTimeAsync(3001);

    const error = await caughtError as Error;
    expect(error.message).toBe("IPC timeout: test_command exceeded 3000ms");

    // Clean up
    await vi.advanceTimersByTimeAsync(60_000);
  });

  it("should respect custom timeout value", async () => {
    vi.mocked(invoke).mockImplementation(
      () => new Promise((resolve) => setTimeout(() => resolve(null), 60_000))
    );

    const promise = invokeWithTimeout("test_command", {}, 5000);

    // Attach rejection handler early to prevent unhandled rejection
    let capturedError: Error | null = null;
    const caughtError = promise.catch((e) => { capturedError = e as Error; });

    // Should NOT timeout at 4999ms — advance to 4999ms
    await vi.advanceTimersByTimeAsync(4999);

    // Verify the promise is still pending (no rejection yet)
    await Promise.resolve(); // flush microtask
    expect(capturedError).toBeNull();

    // Now advance past 5000ms
    await vi.advanceTimersByTimeAsync(2);
    await caughtError;
    expect(capturedError).not.toBeNull();
    expect(capturedError!.message).toBe("IPC timeout: test_command exceeded 5000ms");

    // Clean up
    await vi.advanceTimersByTimeAsync(60_000);
  });

  it("should propagate invoke errors that occur before timeout", async () => {
    const error = new Error("Command failed");
    vi.mocked(invoke).mockRejectedValue(error);

    const promise = invokeWithTimeout("test_command", {}, 3000);
    await expect(promise).rejects.toThrow("Command failed");
  });
});

