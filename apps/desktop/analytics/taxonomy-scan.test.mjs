/**
 * Regression tests for the taxonomy drift scanner (INFRA-905).
 *
 * The security-critical property under test: the scanner never builds shell
 * command strings. All gh invocations are argv arrays passed to execFileSync,
 * so event names originating from PostHog (attacker-influenced input) cannot
 * be shell-interpreted.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  _setGhRunnerForTests,
  computeDrift,
  createDriftIssue,
  driftIssueTitle,
  findExistingIssue,
  isPostHogSystemEvent,
} from './taxonomy-scan.mjs';

const SCANNER_SRC = fs.readFileSync(
  path.join(path.dirname(fileURLToPath(import.meta.url)), 'taxonomy-scan.mjs'),
  'utf-8',
);

test('scanner source never uses shell-based execSync', () => {
  assert.ok(
    !/\bexecSync\s*\(/.test(SCANNER_SRC),
    'execSync(string) spawns a shell and is forbidden; use execFileSync with argv arrays',
  );
  assert.ok(/\bexecFileSync\s*\(/.test(SCANNER_SRC), 'gh calls must go through execFileSync');
});

test('isPostHogSystemEvent flags $-prefixed built-ins only', () => {
  assert.equal(isPostHogSystemEvent('$pageview'), true);
  assert.equal(isPostHogSystemEvent('$autocapture'), true);
  assert.equal(isPostHogSystemEvent('$identify'), true);
  assert.equal(isPostHogSystemEvent('rust_launch'), false);
  assert.equal(isPostHogSystemEvent(''), false);
  assert.equal(isPostHogSystemEvent(null), false);
});

test('computeDrift excludes system events and registered events', () => {
  const online = new Set(['rust_launch', 'typo_evnt', '$pageview', '$identify', 'js_ready']);
  const registered = new Set(['rust_launch', 'js_ready']);
  const { systemEvents, driftEvents } = computeDrift(online, registered);
  assert.deepEqual(systemEvents, ['$identify', '$pageview']);
  assert.deepEqual(driftEvents, ['typo_evnt']);
});

test('findExistingIssue matches on exact canonical title, not substring', () => {
  const calls = [];
  _setGhRunnerForTests((args) => {
    calls.push(args);
    return JSON.stringify([
      { number: 11, title: `Taxonomy drift: unregistered event 'typo'` },
      { number: 12, title: `Taxonomy drift: unregistered event 'typo_extra'` },
    ]);
  });
  try {
    assert.equal(findExistingIssue('typo')?.number, 11);
    // Substring overlap must NOT count as an existing issue
    const near = findExistingIssue('typo_e');
    assert.equal(near, null);
  } finally {
    _setGhRunnerForTests(undefined);
  }

  const searchArg = calls[0][calls[0].indexOf('--search') + 1];
  assert.equal(searchArg, '"typo" in:title');
});

test('findExistingIssue propagates gh failures instead of returning null', () => {
  _setGhRunnerForTests(() => {
    const err = new Error('gh failed');
    err.stderr = Buffer.from('gh: authentication required');
    throw err;
  });
  try {
    assert.throws(() => findExistingIssue('evnt'), /authentication required/);
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('createDriftIssue passes hostile event names as discrete argv, body via file', () => {
  const hostile = `foo'; $(rm -rf /tmp/pwn); \`id\``;
  let recorded;
  let bodyContentAtCall = null;
  let bodyFileExisted = false;
  _setGhRunnerForTests((args) => {
    recorded = args;
    const bodyFile = args[args.indexOf('--body-file') + 1];
    bodyFileExisted = fs.existsSync(bodyFile);
    bodyContentAtCall = fs.readFileSync(bodyFile, 'utf-8');
    return 'https://github.com/example/repo/issues/99\n';
  });
  try {
    const url = createDriftIssue(hostile);
    assert.equal(url, 'https://github.com/example/repo/issues/99');

    // Title is a single discrete argv element — no shell ever sees it
    const titleArg = recorded[recorded.indexOf('--title') + 1];
    assert.equal(titleArg, driftIssueTitle(hostile));
    assert.ok(titleArg.includes('$(rm -rf /tmp/pwn)'));

    // Labels are discrete argv elements
    const labelIdx = recorded.reduce(
      (acc, a, i) => (a === '--label' ? [...acc, recorded[i + 1]] : acc),
      [],
    );
    assert.deepEqual(labelIdx.sort(), ['needs-triage', 'taxonomy-drift']);

    // Body traveled via temp file (existed at call time, cleaned up after)
    assert.ok(bodyFileExisted);
    assert.ok(bodyContentAtCall.includes(hostile));
    const bodyFile = recorded[recorded.indexOf('--body-file') + 1];
    assert.ok(!fs.existsSync(bodyFile), 'temp body file must be cleaned up');

    // No argv element may be a shell command string assembled from the name
    for (const arg of recorded) {
      assert.equal(typeof arg, 'string');
    }
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('createDriftIssue propagates gh failures', () => {
  _setGhRunnerForTests(() => {
    const err = new Error('gh failed');
    err.stderr = Buffer.from("could not add label: 'taxonomy-drift' not found");
    throw err;
  });
  try {
    assert.throws(() => createDriftIssue('evnt'), /not found/);
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('driftIssueTitle format is stable (idempotency contract)', () => {
  assert.equal(driftIssueTitle('x'), "Taxonomy drift: unregistered event 'x'");
});
