#!/usr/bin/env node
// check-imports.mjs — import 封堵脚本
//
// Scans apps/desktop/client/src for .ts/.tsx files and asserts that no file
// imports from "posthog-js" except the whitelisted encapsulation module
// (src/lib/analytics.ts) and its generated type companion / test file.
//
// Checks both static  import ... from "posthog-js"  and dynamic
// import("posthog-js")  patterns.
//
// Exit 0 = pass (no violations), exit 1 = violations found.

import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";

// Resolve src root relative to this script's location
const SCRIPT_DIR = new URL(".", import.meta.url).pathname;
const CLIENT_ROOT = join(SCRIPT_DIR, "..", "client", "src");

// Whitelist: paths (relative to CLIENT_ROOT) allowed to import posthog-js.
//   lib/analytics.ts                         — encapsulation module (INV-4)
//   lib/analytics-events.gen.ts              — codegen type output
//   lib/__tests__/analytics.test.ts          — unit tests mocking posthog-js
const WHITELIST = new Set([
  "lib/analytics.ts",
  "lib/analytics-events.gen.ts",
  "lib/__tests__/analytics.test.ts",
]);

// Pattern: matches  from "posthog-js"  or  import("posthog-js")
const POSTHOG_IMPORT_RE =
  /(?:from\s+["']posthog-js["']|import\s*\(\s*["']posthog-js["']\s*\))/;

async function* walkTsFiles(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  for (const entry of entries) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walkTsFiles(full);
    } else if (/\.(ts|tsx)$/.test(entry.name)) {
      yield full;
    }
  }
}

async function main() {
  const violations = [];

  for await (const filePath of walkTsFiles(CLIENT_ROOT)) {
    const relPath = relative(CLIENT_ROOT, filePath);
    if (WHITELIST.has(relPath)) continue;

    const content = await readFile(filePath, "utf8");
    const lines = content.split("\n");

    for (let i = 0; i < lines.length; i++) {
      if (POSTHOG_IMPORT_RE.test(lines[i])) {
        violations.push({ file: relPath, line: i + 1, text: lines[i].trim() });
      }
    }
  }

  if (violations.length > 0) {
    console.error(
      "❌ posthog-js import gate failed — only src/lib/analytics.ts (and generated/test companions) may import posthog-js:",
    );
    for (const v of violations) {
      console.error(`   ${v.file}:${v.line}  ${v.text}`);
    }
    console.error(
      "\nAll other files must use the typed capture() wrapper from src/lib/analytics.ts.",
    );
    process.exit(1);
  }

  console.log("✅ posthog-js import gate passed — no violations found.");
  process.exit(0);
}

main().catch((err) => {
  console.error("check-imports.mjs: fatal error:", err);
  process.exit(2);
});
