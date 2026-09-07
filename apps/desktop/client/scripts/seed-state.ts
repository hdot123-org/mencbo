#!/usr/bin/env node
/**
 * Seed mock state.json for testing and demonstration.
 * Writes to ~/.mencbo/state.json by default, or MENCBO_STATE_PATH if set.
 */

import { writeFileSync, mkdirSync } from "fs";
import { dirname, resolve } from "path";
import { homedir } from "os";

const state = {
  updated_at: new Date().toISOString(),
  project_id: "hdot123-org/mencbo",
  health: "degraded",
  tasks: {
    "example:heartbeat": {
      display_name: "示例·心跳日志",
      last_run: new Date(Date.now() - 3 * 60 * 1000).toISOString(),
      status: "success",
      duration_ms: 12,
      error: null,
    },
    "example:maintenance": {
      display_name: "示例·维护日志",
      last_run: new Date(Date.now() - 40 * 1000).toISOString(),
      status: "running",
      duration_ms: null,
      error: null,
    },
    "example:failure-demo": {
      display_name: "示例·失败演示",
      last_run: new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString(),
      status: "failed",
      duration_ms: 1234,
      error: "示例失败：故意演示 failed 状态",
    },
  },
};

const targetPath = process.env.MENCBO_STATE_PATH || resolve(homedir(), ".mencbo/state.json");
const targetDir = dirname(targetPath);

mkdirSync(targetDir, { recursive: true });
writeFileSync(targetPath, JSON.stringify(state, null, 2));

console.log(`✓ Wrote mock state to ${targetPath}`);
console.log(`  Health: ${state.health}`);
console.log(`  Tasks: ${Object.keys(state.tasks).length}`);
