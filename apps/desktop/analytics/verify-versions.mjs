#!/usr/bin/env node

/**
 * Version Consistency Verification Script
 *
 * Verifies that version numbers are consistent across four key files:
 * - packages/mencbo/package.json (engine package)
 * - apps/desktop/client/package.json (desktop client)
 * - apps/desktop/client/src-tauri/tauri.conf.json (Tauri config)
 * - apps/desktop/client/src-tauri/Cargo.toml (Rust crate)
 *
 * All four must have the same SemVer version (e.g., "0.2.4").
 * Exit code 0 = all versions match, 1 = mismatch or parse error.
 */

import { readFileSync } from 'fs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const repoRoot = resolve(__dirname, '../../..');

/**
 * Read and parse JSON file
 */
function readJSON(filePath) {
  try {
    const content = readFileSync(filePath, 'utf-8');
    return JSON.parse(content);
  } catch (err) {
    console.error(`ERROR: Failed to read/parse ${filePath}`);
    console.error(err.message);
    process.exit(1);
  }
}

/**
 * Read version from package.json
 */
function readPackageVersion(filePath) {
  const pkg = readJSON(filePath);
  if (!pkg.version) {
    console.error(`ERROR: No "version" field in ${filePath}`);
    process.exit(1);
  }
  return pkg.version;
}

/**
 * Read version from tauri.conf.json
 */
function readTauriVersion(filePath) {
  const conf = readJSON(filePath);
  if (!conf.version) {
    console.error(`ERROR: No "version" field in ${filePath}`);
    process.exit(1);
  }
  return conf.version;
}

/**
 * Read version from Cargo.toml (simple parser for [package] section)
 */
function readCargoVersion(filePath) {
  try {
    const content = readFileSync(filePath, 'utf-8');
    const lines = content.split('\n');

    let inPackageSection = false;
    for (const line of lines) {
      const trimmed = line.trim();

      // Detect section headers
      if (trimmed.startsWith('[')) {
        inPackageSection = trimmed === '[package]';
        continue;
      }

      // Look for version field in [package] section
      if (inPackageSection && trimmed.startsWith('version')) {
        const match = trimmed.match(/^version\s*=\s*"([^"]+)"/);
        if (match) {
          return match[1];
        }
      }
    }

    console.error(`ERROR: No version found in [package] section of ${filePath}`);
    process.exit(1);
  } catch (err) {
    console.error(`ERROR: Failed to read ${filePath}`);
    console.error(err.message);
    process.exit(1);
  }
}

/**
 * Validate SemVer format (basic check)
 */
function isValidSemVer(version) {
  // Simple SemVer pattern: X.Y.Z or X.Y.Z-prerelease
  return /^\d+\.\d+\.\d+(-[a-zA-Z0-9.-]+)?$/.test(version);
}

/**
 * Main verification logic
 */
function verifyVersions() {
  console.log('Verifying version consistency across 4 files...\n');

  // Read versions
  const enginePkg = resolve(repoRoot, 'packages/mencbo/package.json');
  const clientPkg = resolve(repoRoot, 'apps/desktop/client/package.json');
  const tauriConf = resolve(repoRoot, 'apps/desktop/client/src-tauri/tauri.conf.json');
  const cargoToml = resolve(repoRoot, 'apps/desktop/client/src-tauri/Cargo.toml');

  const versions = {
    'packages/mencbo/package.json': readPackageVersion(enginePkg),
    'apps/desktop/client/package.json': readPackageVersion(clientPkg),
    'apps/desktop/client/src-tauri/tauri.conf.json': readTauriVersion(tauriConf),
    'apps/desktop/client/src-tauri/Cargo.toml': readCargoVersion(cargoToml),
  };

  // Display versions
  console.log('Current versions:');
  for (const [file, version] of Object.entries(versions)) {
    console.log(`  ${file}: ${version}`);
  }
  console.log('');

  // Validate SemVer format
  for (const [file, version] of Object.entries(versions)) {
    if (!isValidSemVer(version)) {
      console.error(`ERROR: Invalid SemVer format in ${file}: "${version}"`);
      process.exit(1);
    }
  }

  // Check consistency
  const versionSet = new Set(Object.values(versions));
  if (versionSet.size > 1) {
    console.error('ERROR: Version mismatch detected!');
    console.error('All four files must have the same version.');
    console.error('\nMismatched versions:');
    for (const [file, version] of Object.entries(versions)) {
      console.error(`  ${file}: ${version}`);
    }
    process.exit(1);
  }

  // All good
  const commonVersion = versionSet.values().next().value;
  console.log(`✓ All versions match: ${commonVersion}`);
  process.exit(0);
}

// Run verification
verifyVersions();
