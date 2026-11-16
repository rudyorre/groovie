#!/usr/bin/env node
// Generates Tauri's `latest.json` once from all uploaded draft-release assets so
// parallel OS runners never race to overwrite `latest.json`, and uses direct
// GitHub Releases download URLs instead of rate-limited `api.github.com` URLs.

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const REQUIRED_CLI_ARCHIVES = [
  'grooviectl-windows-x64.zip',
  'grooviectl-windows-arm64.zip',
  'grooviectl-macos-universal.tar.gz',
  'grooviectl-linux-x64.tar.gz',
  'grooviectl-linux-arm64.tar.gz',
];

const REQUIRED_PLATFORMS = [
  'linux-x86_64',
  'linux-x86_64-appimage',
  'linux-x86_64-deb',
  'windows-x86_64',
  'windows-x86_64-nsis',
  'darwin-aarch64',
  'darwin-x86_64',
  'darwin-universal',
];

/**
 * Pure manifest builder used by CI and local verification tests.
 *
 * @param {{
 *   version: string;
 *   tag: string;
 *   repo: string;
 *   notes: string;
 *   pubDate?: string;
 *   assets: Array<{ id?: number; name: string }>;
 *   signatures: Map<string, string>;
 * }} opts
 */
export function buildLatestManifest({
  version,
  tag,
  repo,
  notes,
  pubDate = new Date().toISOString(),
  assets,
  signatures,
}) {
  if (tag !== `v${version}`) {
    throw new Error(`Tag '${tag}' does not match tauri.conf.json version '${version}' (expected 'v${version}')`);
  }

  const assetNames = new Set(assets.map((a) => a.name));

  for (const cliArchive of REQUIRED_CLI_ARCHIVES) {
    if (!assetNames.has(cliArchive)) {
      throw new Error(`Missing required CLI release asset: ${cliArchive}`);
    }
  }

  /** @type {Record<string, { signature: string; url: string }>} */
  const platforms = {};
  const downloadUrl = (name) =>
    `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`;

  const entryFor = (name) => {
    const sig = signatures.get(`${name}.sig`)?.trim();
    if (!sig) return null;
    return { signature: sig, url: downloadUrl(name) };
  };

  for (const { name } of assets) {
    if (name.endsWith('.sig')) continue;

    if (name.endsWith('_amd64.AppImage')) {
      const entry = entryFor(name);
      if (entry) {
        platforms['linux-x86_64'] = entry;
        platforms['linux-x86_64-appimage'] = entry;
      }
    } else if (name.endsWith('_amd64.deb')) {
      const entry = entryFor(name);
      if (entry) {
        platforms['linux-x86_64-deb'] = entry;
      }
    } else if (name.endsWith('_x64-setup.exe')) {
      const entry = entryFor(name);
      if (entry) {
        platforms['windows-x86_64'] = entry;
        platforms['windows-x86_64-nsis'] = entry;
      }
    } else if (name.endsWith('_universal.app.tar.gz')) {
      const entry = entryFor(name);
      if (entry) {
        platforms['darwin-aarch64'] = entry;
        platforms['darwin-x86_64'] = entry;
        platforms['darwin-universal'] = entry;
        platforms['darwin-aarch64-app'] = entry;
        platforms['darwin-x86_64-app'] = entry;
        platforms['darwin-universal-app'] = entry;
      }
    } else if (name.endsWith('_aarch64.app.tar.gz')) {
      const entry = entryFor(name);
      if (entry) {
        platforms['darwin-aarch64'] = entry;
        platforms['darwin-aarch64-app'] = entry;
      }
    } else if (name.endsWith('_x64.app.tar.gz')) {
      const entry = entryFor(name);
      if (entry) {
        platforms['darwin-x86_64'] = entry;
        platforms['darwin-x86_64-app'] = entry;
      }
    }
  }

  for (const target of REQUIRED_PLATFORMS) {
    if (!platforms[target]) {
      throw new Error(`Missing required updater platform '${target}' in release assets`);
    }
  }

  return {
    version,
    notes,
    pub_date: pubDate,
    platforms,
  };
}

function ghApi(args) {
  return execFileSync('gh', ['api', ...args], {
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
}

function main() {
  const repo = process.env.GITHUB_REPOSITORY;
  const tag = process.env.TAG;
  const releaseId = process.env.RELEASE_ID;
  const outPath = process.env.OUTPUT_PATH || 'latest.json';

  if (!repo || !tag || !releaseId) {
    throw new Error('Expected GITHUB_REPOSITORY, TAG, and RELEASE_ID environment variables');
  }

  const tauriConf = JSON.parse(readFileSync('apps/desktop/src-tauri/tauri.conf.json', 'utf8'));
  const release = JSON.parse(ghApi([`repos/${repo}/releases/${releaseId}`]));
  const assets = JSON.parse(ghApi(['--paginate', `repos/${repo}/releases/${releaseId}/assets`]));

  const signatures = new Map();
  for (const asset of assets) {
    if (!asset.name.endsWith('.sig')) continue;
    const sig = ghApi(['-H', 'Accept: application/octet-stream', `repos/${repo}/releases/assets/${asset.id}`]);
    signatures.set(asset.name, sig.trim());
  }

  const manifest = buildLatestManifest({
    version: tauriConf.version,
    tag,
    repo,
    notes: release.body || '',
    assets,
    signatures,
  });

  writeFileSync(outPath, JSON.stringify(manifest, null, 2) + '\n');
  console.log(`Wrote ${outPath} for ${tag} with ${Object.keys(manifest.platforms).length} platform entries.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
