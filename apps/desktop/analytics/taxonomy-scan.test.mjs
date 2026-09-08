/**
 * Regression tests for the taxonomy drift scanner (INFRA-905, INFRA-908).
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
  closeDriftIssue,
  computeDrift,
  createDriftIssue,
  driftIssueTitle,
  findExistingIssue,
  isPostHogSystemEvent,
  listOpenDriftIssues,
  parseDriftEventName,
  resolveDriftIssueAction,
  reopenDriftIssue,
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

test('parseDriftEventName inverts driftIssueTitle exactly', () => {
  assert.equal(parseDriftEventName(driftIssueTitle('rust_launch')), 'rust_launch');
  // Hostile names round-trip: quotes stay part of one argv-safe token
  assert.equal(parseDriftEventName(driftIssueTitle(`a'; $(id)`)), `a'; $(id)`);
  // Foreign or malformed titles are rejected (never lifecycle-managed)
  assert.equal(parseDriftEventName('Some other title'), null);
  assert.equal(parseDriftEventName('Taxonomy drift: unregistered event'), null);
  assert.equal(parseDriftEventName("Taxonomy drift: unregistered event ''"), null);
  assert.equal(parseDriftEventName('Taxonomy drift: unregistered event typo'), null);
  assert.equal(parseDriftEventName(null), null);
  assert.equal(parseDriftEventName(42), null);
});

test('resolveDriftIssueAction keeps issues for ongoing drift', () => {
  const action = resolveDriftIssueAction('typo_evnt', {
    driftSet: new Set(['typo_evnt']),
    registeredSet: new Set(),
    online7d: new Set(['typo_evnt']),
  });
  assert.equal(action, null);
});

test('resolveDriftIssueAction closes by registration with an explanatory reason', () => {
  const action = resolveDriftIssueAction('new_event', {
    driftSet: new Set(),
    registeredSet: new Set(['new_event']),
    online7d: new Set(['new_event']),
  });
  assert.ok(action.close.includes('registered'));
  assert.ok(action.close.includes('new_event'));
});

test('resolveDriftIssueAction closes when the event is no longer observed', () => {
  const action = resolveDriftIssueAction('drift_probe_r5_1788910013', {
    driftSet: new Set(),
    registeredSet: new Set(),
    online7d: new Set(),
  });
  assert.ok(action.close.includes('no longer observed'));
  assert.ok(action.close.includes('drift_probe_r5_1788910013'));
});

test('resolveDriftIssueAction keeps issues for observed-but-excluded events', () => {
  // e.g. a manually-filed issue for a $ system event: observed online,
  // excluded from drift, unregistered — the scanner must not close an
  // issue for an event it does not track.
  const action = resolveDriftIssueAction('$pageview', {
    driftSet: new Set(),
    registeredSet: new Set(),
    online7d: new Set(['$pageview']),
  });
  assert.equal(action, null);
});

test('listOpenDriftIssues queries open issues with the drift label', () => {
  let recorded;
  _setGhRunnerForTests((args) => {
    recorded = args;
    return JSON.stringify([
      { number: 79, title: driftIssueTitle('drift_probe_r4_xyz') },
      { number: 82, title: driftIssueTitle('drift_probe_r5_1788910013') },
    ]);
  });
  try {
    const issues = listOpenDriftIssues();
    assert.equal(issues.length, 2);
    assert.equal(recorded[recorded.indexOf('--state') + 1], 'open');
    assert.equal(recorded[recorded.indexOf('--label') + 1], 'taxonomy-drift');
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('listOpenDriftIssues propagates gh failures', () => {
  _setGhRunnerForTests(() => {
    const err = new Error('gh failed');
    err.stderr = Buffer.from('gh: Could not parse json');
    throw err;
  });
  try {
    assert.throws(() => listOpenDriftIssues(), /Could not parse json/);
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('closeDriftIssue passes number and comment as discrete argv', () => {
  let recorded;
  _setGhRunnerForTests((args) => {
    recorded = args;
    return '';
  });
  try {
    closeDriftIssue(82, 'Event `x; y` $(id) is now registered in `events.yml`; drift resolved.');
    assert.equal(recorded[recorded.indexOf('close') + 1], '82');
    // Comment with shell metacharacters stays a single argv element
    const comment = recorded[recorded.indexOf('--comment') + 1];
    assert.ok(comment.includes('`x; y` $(id)'));
    assert.ok(comment.includes('_Closed by taxonomy-scan workflow_'));
    for (const arg of recorded) {
      assert.equal(typeof arg, 'string');
    }
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('closeDriftIssue propagates gh failures', () => {
  _setGhRunnerForTests(() => {
    const err = new Error('gh failed');
    err.stderr = Buffer.from('gh: issue close failed: permission denied');
    throw err;
  });
  try {
    assert.throws(() => closeDriftIssue(82, 'reason'), /permission denied/);
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('reopenDriftIssue passes hostile event names as discrete argv', () => {
  const hostile = `foo'; $(rm -rf /tmp/pwn); \`id\``;
  let recorded;
  _setGhRunnerForTests((args) => {
    recorded = args;
    return '';
  });
  try {
    reopenDriftIssue(11, hostile);
    assert.equal(recorded[recorded.indexOf('reopen') + 1], '11');
    const comment = recorded[recorded.indexOf('--comment') + 1];
    assert.ok(comment.includes(hostile));
    assert.ok(comment.includes('_Reopened by taxonomy-scan workflow_'));
    for (const arg of recorded) {
      assert.equal(typeof arg, 'string');
    }
  } finally {
    _setGhRunnerForTests(undefined);
  }
});

test('findExistingIssue surfaces issue state for lifecycle decisions', () => {
  _setGhRunnerForTests(() =>
    JSON.stringify([{ number: 13, title: driftIssueTitle('gone_evnt'), state: 'CLOSED' }]),
  );
  try {
    const existing = findExistingIssue('gone_evnt');
    assert.equal(existing?.number, 13);
    assert.equal(existing?.state, 'CLOSED');
  } finally {
    _setGhRunnerForTests(undefined);
  }
});
