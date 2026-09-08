#!/usr/bin/env node
/**
 * @fileoverview Taxonomy drift scanner
 * Compares PostHog online events against events.yml registry.
 *
 * Modes:
 * - Live: POSTHOG_PERSONAL_API_KEY set + DRY_RUN != 'true'
 *   Queries PostHog, reports drift/dead/silent, creates GitHub issues for
 *   drift, and manages the drift-issue lifecycle (INFRA-908): open issues
 *   whose event has been registered or is no longer observed are closed
 *   with an explanatory comment; closed issues whose event drifts again
 *   are reopened instead of being silently suppressed.
 * - Dry-run: secret missing or DRY_RUN=true
 *   Queries PostHog if key available but never creates/closes/reopens
 *   issues. If key missing, outputs registry summary only.
 *
 * Scanner discipline: no automatic PRs — issues only.
 *
 * Security invariants (INFRA-905):
 * - gh CLI is invoked exclusively via execFileSync with argv arrays; the
 *   scanner must never build shell command strings (no execSync), so event
 *   names from PostHog can never be shell-interpreted.
 * - Live-mode issue creation runs an auth pre-flight and fails loudly
 *   (non-zero exit) instead of silently swallowing credential errors.
 * - Idempotency lookups distinguish "checked, none found" from "check
 *   failed"; a failed check aborts that event's issue creation rather than
 *   risking duplicates.
 * - Lifecycle mutations (close/reopen) apply only to issues whose title
 *   matches the canonical drift format exactly; foreign issues carrying
 *   the taxonomy-drift label are left untouched.
 */

import fs from 'fs';
import os from 'os';
import path from 'path';
import { execFileSync } from 'child_process';
import { fileURLToPath } from 'url';
import yaml from 'yaml';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const EVENTS_YML = path.join(REPO_ROOT, 'apps/desktop/analytics/events.yml');
const POSTHOG_HOST = 'https://us.posthog.com';
const POSTHOG_PROJECT_ID = 597439;

// Explicit repo target in CI (GITHUB_REPOSITORY is always set on Actions);
// locally gh resolves the repo from the git remote.
const GH_REPO = process.env.GITHUB_REPOSITORY || null;

const DRIFT_LABELS = [
  { name: 'taxonomy-drift', color: 'e3b341', description: 'Taxonomy drift detected by taxonomy-scan workflow' },
  { name: 'needs-triage', color: 'd876e3', description: 'Awaiting triage' },
];

/**
 * gh runner, overridable in tests. Always argv-array based — no shell.
 */
const defaultGhRunner = (args) =>
  execFileSync('gh', args, { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });

let ghRunner = defaultGhRunner;

export function _setGhRunnerForTests(fn) {
  ghRunner = fn || defaultGhRunner;
}

function ghRepoArgs() {
  return GH_REPO ? ['--repo', GH_REPO] : [];
}

function ghExec(args) {
  try {
    return ghRunner([...ghRepoArgs(), ...args]).trim();
  } catch (err) {
    throw new Error(`gh ${args[0]} failed: ${describeGhError(err)}`);
  }
}

/**
 * Extract a readable message from a gh failure (stderr carries the actual
 * reason, e.g. auth/permission errors).
 */
function describeGhError(err) {
  const stderr = err && err.stderr ? Buffer.from(err.stderr).toString('utf-8').trim() : '';
  return stderr || (err && err.message) || String(err);
}

/**
 * Query PostHog HogQL API
 */
async function queryPostHog(apiKey, hogql) {
  const res = await fetch(`${POSTHOG_HOST}/api/projects/${POSTHOG_PROJECT_ID}/query/`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${apiKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      query: { kind: 'HogQLQuery', query: hogql },
    }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`PostHog API ${res.status}: ${text}`);
  }
  return res.json();
}

/**
 * Get distinct event names seen within last N days
 */
async function getOnlineEventNames(apiKey, days) {
  if (!Number.isInteger(days) || days <= 0) {
    throw new Error(`Invalid interval (expected positive integer): ${days}`);
  }
  const result = await queryPostHog(
    apiKey,
    `SELECT event FROM events WHERE timestamp > now() - INTERVAL ${days} DAY GROUP BY event`,
  );
  const names = new Set();
  if (result.results) {
    for (const row of result.results) {
      if (row[0]) names.add(row[0]);
    }
  }
  return names;
}

/**
 * PostHog built-in / system events (e.g. $pageview, $identify,
 * $autocapture, $create_alias, $groupidentify, $merge_duplicated_people).
 * They are not user-defined and never belong in events.yml, so they are
 * excluded from drift to prevent noisy false positives.
 */
export function isPostHogSystemEvent(name) {
  return typeof name === 'string' && name.startsWith('$');
}

/**
 * Compute drift from online events vs registered names.
 * Returns: { systemEvents: string[], driftEvents: string[] } (sorted)
 */
export function computeDrift(onlineEvents, registeredNames) {
  const systemEvents = [...onlineEvents].filter(isPostHogSystemEvent).sort();
  const driftEvents = [...onlineEvents]
    .filter((e) => !registeredNames.has(e) && !isPostHogSystemEvent(e))
    .sort();
  return { systemEvents, driftEvents };
}

/**
 * Parse events.yml registry
 * Returns: { active: Set<string>, deprecated: Map<string, string[]>, allRegistered: Set<string> }
 */
function parseRegistry() {
  if (!fs.existsSync(EVENTS_YML)) {
    throw new Error(`events.yml not found at ${EVENTS_YML}`);
  }

  const content = fs.readFileSync(EVENTS_YML, 'utf-8');
  const data = yaml.parse(content);

  if (!data.schema_version) {
    throw new Error('events.yml missing schema_version');
  }

  const active = new Set();
  const deprecated = new Map();

  for (const [name, event] of Object.entries(data.events || {})) {
    if (event.status === 'active') {
      active.add(name);
    } else if (event.status === 'deprecated') {
      // Handle both string and array forms of replaced_by
      const replacedBy = event.replaced_by;
      if (Array.isArray(replacedBy)) {
        deprecated.set(name, replacedBy);
      } else if (typeof replacedBy === 'string') {
        deprecated.set(name, [replacedBy]);
      } else {
        deprecated.set(name, []);
      }
    }
  }

  const allRegistered = new Set([...active, ...deprecated.keys()]);
  return { active, deprecated, allRegistered };
}

/**
 * Canonical drift issue title (single definition keeps idempotency exact)
 */
export function driftIssueTitle(eventName) {
  return `Taxonomy drift: unregistered event '${eventName}'`;
}

/**
 * Inverse of driftIssueTitle: extract the event name from a canonical drift
 * issue title. Returns null for titles that do not match the canonical
 * format exactly — foreign issues carrying the taxonomy-drift label must
 * never be lifecycle-managed by the scanner (INFRA-908).
 */
export function parseDriftEventName(title) {
  if (typeof title !== 'string') return null;
  const prefix = "Taxonomy drift: unregistered event '";
  if (!title.startsWith(prefix) || !title.endsWith("'")) return null;
  const name = title.slice(prefix.length, title.length - 1);
  return name.length > 0 ? name : null;
}

/**
 * Decide the lifecycle action for an open drift issue given current scan
 * data (INFRA-908). Returns null when the issue must stay open, or
 * { close: <reason> } when the reported drift has resolved:
 * - event still in the current drift set -> keep (drift ongoing)
 * - event registered (active or deprecated) -> close (resolved by registration)
 * - event observed online but excluded from drift (e.g. $ system event) ->
 *   keep (the scanner does not track it; leave the issue to humans)
 * - event absent from the 7-day online window -> close (emission stopped
 *   or transient probe aged out)
 */
export function resolveDriftIssueAction(eventName, { driftSet, registeredSet, online7d }) {
  if (driftSet.has(eventName)) return null;
  if (registeredSet.has(eventName)) {
    return {
      close: `Event \`${eventName}\` is now registered in \`events.yml\`; drift resolved.`,
    };
  }
  if (online7d.has(eventName)) return null;
  return {
    close: `Event \`${eventName}\` is no longer observed in PostHog (last 7 days); drift resolved.`,
  };
}

/**
 * Check if a GitHub issue already exists for a drift event (idempotent).
 * Returns the issue's state so the caller can distinguish "open, skip"
 * from "closed, reopen" (INFRA-908).
 * Throws on gh failure (auth, network, permissions) so the caller can
 * distinguish "checked, none found" from "check failed".
 */
export function findExistingIssue(eventName) {
  const raw = ghExec([
    'issue', 'list',
    '--state', 'all',
    '--label', 'taxonomy-drift',
    '--search', `"${eventName}" in:title`,
    '--json', 'number,title,state',
    '--limit', '20',
  ]);
  const issues = JSON.parse(raw || '[]');
  const target = driftIssueTitle(eventName);
  return issues.find((i) => i.title === target) || null;
}

/**
 * Pre-flight for live mode: exercises gh auth + repo resolution + issues
 * read in one round trip. Throws with a readable message on failure.
 */
function ghPreflight() {
  ghExec(['issue', 'list', '--label', 'taxonomy-drift', '--state', 'all', '--json', 'number', '--limit', '1']);
}

/**
 * Ensure drift labels exist (gh issue create fails on unknown labels).
 * Idempotent: --force updates in place when the label already exists.
 */
function ensureDriftLabels() {
  for (const label of DRIFT_LABELS) {
    ghExec([
      'label', 'create', label.name,
      '--color', label.color,
      '--description', label.description,
      '--force',
    ]);
  }
}

/**
 * Create a GitHub issue for a drift event.
 * Title and labels are passed as discrete argv elements (no shell), and the
 * body travels via a temp file, so hostile event names cannot inject.
 * Throws on gh failure.
 */
export function createDriftIssue(eventName) {
  const title = driftIssueTitle(eventName);
  const body = [
    `Event \`${eventName}\` was observed in PostHog (last 7 days) but is not registered in \`events.yml\`.`,
    '',
    'This may be a typo or an unregistered emission point. Please verify and either:',
    '1. Add the event to `events.yml` if intentional',
    '2. Fix the emission point if it\'s a typo',
    '',
    '_Created by taxonomy-scan workflow_',
  ].join('\n');

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'taxonomy-drift-'));
  const tmpFile = path.join(dir, 'issue-body.md');
  try {
    fs.writeFileSync(tmpFile, body);
    return ghExec([
      'issue', 'create',
      '--title', title,
      '--label', 'taxonomy-drift',
      '--label', 'needs-triage',
      '--body-file', tmpFile,
    ]);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

/**
 * List open issues labeled taxonomy-drift. Throws on gh failure.
 */
export function listOpenDriftIssues() {
  const raw = ghExec([
    'issue', 'list',
    '--state', 'open',
    '--label', 'taxonomy-drift',
    '--json', 'number,title',
    '--limit', '100',
  ]);
  return JSON.parse(raw || '[]');
}

/**
 * Close a resolved drift issue with an explanatory comment (INFRA-908).
 * The comment travels as a single discrete argv element — no shell.
 * Throws on gh failure.
 */
export function closeDriftIssue(issueNumber, reason) {
  return ghExec([
    'issue', 'close', String(issueNumber),
    '--comment', `${reason}\n\n_Closed by taxonomy-scan workflow_`,
  ]);
}

/**
 * Reopen a previously closed drift issue whose event drifted again
 * (INFRA-908). Without this, the state-all idempotency lookup would
 * permanently suppress re-reports for events that reappear later.
 * Throws on gh failure.
 */
export function reopenDriftIssue(issueNumber, eventName) {
  return ghExec([
    'issue', 'reopen', String(issueNumber),
    '--comment',
    `Event \`${eventName}\` is observed in PostHog again (last 7 days) but still unregistered; reopening.\n\n_Reopened by taxonomy-scan workflow_`,
  ]);
}

/**
 * Main scanner logic
 */
async function main() {
  const apiKey = process.env.POSTHOG_PERSONAL_API_KEY;
  const explicitDryRun = process.env.DRY_RUN === 'true';
  const dryRun = explicitDryRun || !apiKey;
  const isGitHubActions = !!process.env.GITHUB_ACTIONS;

  const lines = [];
  const log = (msg) => {
    console.log(msg);
    lines.push(msg);
  };

  log('=== Taxonomy Drift Scan ===');
  log(`Mode: ${dryRun ? 'dry-run' : 'live'}`);
  log(`PostHog API key: ${apiKey ? 'configured' : 'NOT configured'}`);
  if (explicitDryRun && apiKey) {
    log('(dry-run requested via workflow_dispatch input)');
  }
  log('');

  // Parse registry
  const { active, deprecated, allRegistered } = parseRegistry();
  log(`Registry: ${active.size} active, ${deprecated.size} deprecated, ${allRegistered.size} total`);
  log('');

  // If no API key, output registry summary and exit
  if (!apiKey) {
    log('WARNING: POSTHOG_PERSONAL_API_KEY not configured - degrading to dry-run mode');
    log('Cannot query PostHog for online event data.');
    log('');
    log('### Active Events (registered):');
    for (const name of [...active].sort()) {
      log(`  - ${name}`);
    }
    log('');
    log('### Deprecated Events:');
    for (const [name, replacements] of [...deprecated.entries()].sort()) {
      log(`  - ${name} -> ${replacements.join(', ')}`);
    }
    log('');
    log('To enable live scanning, configure POSTHOG_PERSONAL_API_KEY secret (scope: query:read)');
    writeStepSummary(lines, isGitHubActions);
    return;
  }

  // Query PostHog for online events
  log('Querying PostHog for online events...');
  let online7d, online30d, online90d;
  try {
    [online7d, online30d, online90d] = await Promise.all([
      getOnlineEventNames(apiKey, 7),
      getOnlineEventNames(apiKey, 30),
      getOnlineEventNames(apiKey, 90),
    ]);
  } catch (err) {
    log(`ERROR: PostHog query failed: ${err.message}`);
    log('Falling back to dry-run mode for this run.');
    writeStepSummary(lines, isGitHubActions);
    return;
  }

  log(`Online events: ${online7d.size} (7d), ${online30d.size} (30d), ${online90d.size} (90d)`);
  log('');

  // Compute drift: events online but not registered
  const { systemEvents, driftEvents } = computeDrift(online7d, allRegistered);

  if (systemEvents.length > 0) {
    log(`Excluded PostHog system events (${systemEvents.length}): ${systemEvents.join(', ')}`);
  }

  log(`### Drift Events (online 7d but NOT registered): ${driftEvents.length}`);
  if (driftEvents.length > 0) {
    for (const e of driftEvents) {
      log(`  [DRIFT] ${e}`);
    }
  } else {
    log('  No drift detected');
  }
  log('');

  // Compute dead events: active but zero traffic in 30 days
  const deadEvents = [...active].filter((e) => !online30d.has(e)).sort();
  log(`### Dead Events (active, zero traffic 30d): ${deadEvents.length}`);
  if (deadEvents.length > 0) {
    for (const e of deadEvents) {
      log(`  [DEAD] ${e}`);
    }
  } else {
    log('  All active events have traffic');
  }
  log('');

  // Compute silent deprecated: deprecated, not seen in 90 days
  const silentDeprecated = [...deprecated.keys()].filter((e) => !online90d.has(e)).sort();
  log(`### Silent Deprecated (not seen 90d): ${silentDeprecated.length}`);
  if (silentDeprecated.length > 0) {
    for (const e of silentDeprecated) {
      log(`  [SILENT] ${e}`);
    }
  } else {
    log('  All deprecated events still have recent traffic (or none are deprecated)');
  }
  log('');

  // Issue lifecycle management (live mode only, INFRA-908):
  // pass 1 creates (or reopens) issues for currently drifting events,
  // pass 2 closes open drift issues whose drift has resolved.
  let closedCount = 0;
  let reopenedCount = 0;
  if (!dryRun) {
    // Pre-flight: verify auth + repo + issue read before any mutation.
    // A credential failure here must fail the run loudly (INFRA-905).
    try {
      ghPreflight();
      ensureDriftLabels();
    } catch (err) {
      log(`ERROR: GitHub pre-flight failed (auth/repo/labels): ${describeGhError(err)}`);
      log('Aborting issue lifecycle management. Verify GH_TOKEN and issues: write permission, then re-run.');
      process.exitCode = 1;
      return;
    }

    let failures = 0;

    // Pass 1: create or reopen issues for currently drifting events
    if (driftEvents.length > 0) {
      log('Creating GitHub issues for drift events...');
      for (const eventName of driftEvents) {
        let existing;
        try {
          existing = findExistingIssue(eventName);
        } catch (err) {
          failures += 1;
          log(`  [FAIL] idempotency check for "${eventName}": ${describeGhError(err)}`);
          continue;
        }
        if (existing) {
          if (existing.state === 'OPEN') {
            log(`  [SKIP] Issue #${existing.number} already exists for "${eventName}"`);
          } else {
            try {
              reopenDriftIssue(existing.number, eventName);
              reopenedCount += 1;
              log(`  [REOPEN] Issue #${existing.number} reopened for "${eventName}"`);
            } catch (err) {
              failures += 1;
              log(`  [FAIL] reopen #${existing.number} for "${eventName}": ${describeGhError(err)}`);
            }
          }
          continue;
        }
        try {
          const url = createDriftIssue(eventName);
          log(`  [CREATED] ${url}`);
        } catch (err) {
          failures += 1;
          log(`  [FAIL] create issue for "${eventName}": ${describeGhError(err)}`);
        }
      }
      log('');
    }

    // Pass 2: close open drift issues whose drift has resolved
    log('Checking open drift issues for resolution...');
    let openIssues;
    try {
      openIssues = listOpenDriftIssues();
    } catch (err) {
      failures += 1;
      log(`  [FAIL] list open drift issues: ${describeGhError(err)}`);
      openIssues = [];
    }
    const driftSet = new Set(driftEvents);
    for (const issue of openIssues) {
      const eventName = parseDriftEventName(issue.title);
      if (!eventName) {
        log(`  [SKIP] Issue #${issue.number} has a non-canonical title - leaving untouched`);
        continue;
      }
      const action = resolveDriftIssueAction(eventName, { driftSet, registeredSet: allRegistered, online7d });
      if (!action) {
        log(`  [KEEP] Issue #${issue.number} for "${eventName}" (drift ongoing)`);
        continue;
      }
      try {
        closeDriftIssue(issue.number, action.close);
        closedCount += 1;
        log(`  [CLOSED] #${issue.number} for "${eventName}"`);
      } catch (err) {
        failures += 1;
        log(`  [FAIL] close #${issue.number} for "${eventName}": ${describeGhError(err)}`);
      }
    }
    log('');

    if (failures > 0) {
      log(`ERROR: ${failures} drift issue lifecycle operation(s) failed in live mode`);
      process.exitCode = 1;
    }
  } else if (dryRun && driftEvents.length > 0) {
    log('Dry-run mode: skipping issue creation and lifecycle management');
  }

  // Write GitHub Actions step summary
  if (isGitHubActions) {
    const summaryLines = [
      '# Taxonomy Drift Scan Report',
      '',
      `**Mode**: ${dryRun ? 'dry-run' : 'live'}`,
      '',
      `## Drift Events (${driftEvents.length})`,
      '',
      driftEvents.length > 0
        ? driftEvents.map((e) => `- \`${e}\``).join('\n')
        : '_No drift detected_',
      '',
      `## Dead Events (${deadEvents.length})`,
      '',
      deadEvents.length > 0
        ? deadEvents.map((e) => `- \`${e}\``).join('\n')
        : '_All active events have traffic_',
      '',
      `## Silent Deprecated (${silentDeprecated.length})`,
      '',
      silentDeprecated.length > 0
        ? silentDeprecated.map((e) => `- \`${e}\``).join('\n')
        : '_All deprecated events still have recent traffic_',
      '',
      '## Drift Issue Lifecycle',
      '',
      dryRun
        ? '_Dry-run: issue lifecycle not attempted_'
        : `${closedCount} resolved issue(s) closed, ${reopenedCount} reopened`,
      '',
    ].join('\n');

    const summaryPath = process.env.GITHUB_STEP_SUMMARY;
    if (summaryPath) {
      fs.appendFileSync(summaryPath, summaryLines + '\n');
    }
  }
}

function writeStepSummary(lines, isGitHubActions) {
  if (!isGitHubActions) return;
  const summaryPath = process.env.GITHUB_STEP_SUMMARY;
  if (!summaryPath) return;

  const summary = [
    '# Taxonomy Drift Scan Report (dry-run)',
    '',
    'PostHog API key not configured. Only registry summary available.',
    '',
    lines.filter((l) => l.startsWith('###') || l.startsWith('  -')).join('\n'),
    '',
  ].join('\n');

  fs.appendFileSync(summaryPath, summary + '\n');
}

// Run only when executed directly (imports for tests must not trigger a scan)
const isDirectRun =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isDirectRun) {
  main().catch((err) => {
    console.error('Scanner failed:', err.message);
    process.exit(1);
  });
}
