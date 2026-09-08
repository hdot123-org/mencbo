#!/usr/bin/env node
/**
 * @fileoverview Taxonomy drift scanner
 * Compares PostHog online events against events.yml registry.
 * 
 * Modes:
 * - Live: POSTHOG_PERSONAL_API_KEY set + DRY_RUN != 'true'
 *   Queries PostHog, reports drift/dead/silent, creates GitHub issues for drift.
 * - Dry-run: secret missing or DRY_RUN=true
 *   Queries PostHog if key available but never creates issues.
 *   If key missing, outputs registry summary only.
 * 
 * Scanner discipline: no automatic PRs — issues only.
 */

import fs from 'fs';
import path from 'path';
import { execSync } from 'child_process';
import yaml from 'yaml';

const REPO_ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');
const EVENTS_YML = path.join(REPO_ROOT, 'apps/desktop/analytics/events.yml');
const POSTHOG_HOST = 'https://us.posthog.com';
const POSTHOG_PROJECT_ID = 597439;

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
 * Check if a GitHub issue already exists for a drift event (idempotent)
 */
function findExistingIssue(eventName) {
  try {
    const raw = execSync(
      `gh issue list --state all --label taxonomy-drift --search "${eventName}" --json number,title --limit 5`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim();
    const issues = JSON.parse(raw || '[]');
    // Exact match on title pattern
    return issues.find(
      (i) => i.title && i.title.includes(eventName),
    );
  } catch {
    return null;
  }
}

/**
 * Create a GitHub issue for a drift event
 */
function createDriftIssue(eventName) {
  const title = `Taxonomy drift: unregistered event '${eventName}'`;
  const body = [
    `Event \`${eventName}\` was observed in PostHog (last 7 days) but is not registered in \`events.yml\`.`,
    '',
    'This may be a typo or an unregistered emission point. Please verify and either:',
    '1. Add the event to `events.yml` if intentional',
    '2. Fix the emission point if it\'s a typo',
    '',
    '_Created by taxonomy-scan workflow_',
  ].join('\n');

  try {
    const result = execSync(
      `gh issue create --title ${JSON.stringify(title)} --label taxonomy-drift,needs-triage --body ${JSON.stringify(body)}`,
      { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] },
    ).trim();
    return result;
  } catch (err) {
    return `ERROR: ${err.message}`;
  }
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
  const driftEvents = [...online7d].filter((e) => !allRegistered.has(e)).sort();
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

  // Issue creation for drift events (live mode only)
  if (!dryRun && driftEvents.length > 0) {
    log('Creating GitHub issues for drift events...');
    for (const eventName of driftEvents) {
      const existing = findExistingIssue(eventName);
      if (existing) {
        log(`  [SKIP] Issue #${existing.number} already exists for "${eventName}"`);
        continue;
      }
      const result = createDriftIssue(eventName);
      if (result.startsWith('ERROR:')) {
        log(`  [FAIL] "${eventName}": ${result}`);
      } else {
        log(`  [CREATED] ${result}`);
      }
    }
  } else if (dryRun && driftEvents.length > 0) {
    log('Dry-run mode: skipping issue creation');
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

main().catch((err) => {
  console.error('Scanner failed:', err.message);
  process.exit(1);
});
