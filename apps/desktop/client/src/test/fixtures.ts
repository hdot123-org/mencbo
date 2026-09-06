import type { State } from "../types";

/**
 * Mock state fixtures for browser dev mode.
 * Time references are relative to page load (now).
 */
const now = new Date();

export const MOCK_STATE: State = {
  mock: true,
  tasks: [
    {
      id: "example:heartbeat",
      name: "示例·心跳日志",
      status: "success",
      lastRun: new Date(now.getTime() - 3 * 60 * 1000).toISOString(),
      durationMs: 12,
    },
    {
      id: "example:maintenance",
      name: "示例·维护日志",
      status: "running",
      lastRun: new Date(now.getTime() - 40 * 1000).toISOString(),
      durationMs: null,
    },
    {
      id: "example:failure-demo",
      name: "示例·失败演示",
      status: "failed",
      lastRun: new Date(now.getTime() - 2 * 60 * 60 * 1000).toISOString(),
      durationMs: 1234,
      error: "示例失败：故意演示 failed 状态",
    },
  ],
};

export const MOCK_STATE_OK: State = {
  mock: true,
  tasks: [
    {
      id: "example:heartbeat",
      name: "示例·心跳日志",
      status: "success",
      lastRun: new Date(now.getTime() - 3 * 60 * 1000).toISOString(),
      durationMs: 12,
    },
    {
      id: "example:maintenance",
      name: "示例·维护日志",
      status: "running",
      lastRun: new Date(now.getTime() - 40 * 1000).toISOString(),
      durationMs: null,
    },
  ],
};

export const MOCK_STATE_EMPTY: State = {
  mock: true,
  tasks: [],
};
