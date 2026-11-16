#!/usr/bin/env node
// Generates `altstore-source.json` compatible with both SideStore and AltStore so
// users can add `https://github.com/<owner>/<repo>/releases/latest/download/altstore-source.json`
// once and get 1-tap installs & updates signed with their free Apple ID.

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const LOCAL_NETWORK_REASON =
  'Groovie discovers and controls your Govee lights directly over your local Wi-Fi network without cloud servers.';

/**
 * Finds the iOS `.ipa` asset in a GitHub Release's asset list.
 *
 * @param {Array<{ name: string; size: number }>} assets
 */
function findIosIpaAsset(assets) {
  return assets.find((a) => a.name.endsWith('_ios.ipa') || a.name.endsWith('.ipa')) ?? null;
}

/**
 * Pure builder for the SideStore / AltStore source JSON manifest.
 *
 * @param {{
 *   version: string;
 *   bundleIdentifier: string;
 *   tag: string;
 *   repo: string;
 *   notes: string;
 *   pubDate?: string;
 *   currentAssets: Array<{ name: string; size: number }>;
 *   previousReleases?: Array<{
 *     tag_name: string;
 *     draft?: boolean;
 *     prerelease?: boolean;
 *     published_at?: string | null;
 *     created_at?: string | null;
 *     body?: string | null;
 *     assets?: Array<{ name: string; size: number }>;
 *   }>;
 * }} opts
 */
export function buildAltStoreSource({
  version,
  bundleIdentifier,
  tag,
  repo,
  notes,
  pubDate = new Date().toISOString(),
  currentAssets,
  previousReleases = [],
}) {
  if (tag !== `v${version}`) {
    throw new Error(
      `Tag '${tag}' does not match apps/ios/src-tauri/tauri.conf.json version '${version}' (expected 'v${version}')`
    );
  }

  const currentIpa = findIosIpaAsset(currentAssets);
  if (!currentIpa) {
    throw new Error(`Missing required iOS .ipa asset (e.g. Groovie_${version}_ios.ipa) in release ${tag}`);
  }
  if (!Number.isFinite(currentIpa.size) || currentIpa.size <= 0) {
    throw new Error(`Invalid byte size (${currentIpa.size}) for iOS asset ${currentIpa.name}`);
  }

  const downloadUrl = (releaseTag, assetName) =>
    `https://github.com/${repo}/releases/download/${encodeURIComponent(releaseTag)}/${encodeURIComponent(assetName)}`;

  const cleanNotes = (raw, ver) => {
    const trimmed = (raw ?? '').trim();
    return trimmed || `Groovie v${ver} for iOS.`;
  };

  /** @type {Array<{ version: string; date: string; localizedDescription: string; downloadURL: string; size: number; minOSVersion: string }>} */
  const versions = [
    {
      version,
      date: pubDate,
      localizedDescription: cleanNotes(notes, version),
      downloadURL: downloadUrl(tag, currentIpa.name),
      size: currentIpa.size,
      minOSVersion: '15.0',
    },
  ];

  const seenVersions = new Set([version]);
  const includePrerelease = tag.includes('-');

  for (const rel of previousReleases) {
    if (!rel || rel.draft) continue;
    if (rel.prerelease && !includePrerelease) continue;
    if (!rel.tag_name || !rel.tag_name.startsWith('v')) continue;
    const relVersion = rel.tag_name.slice(1);
    if (!relVersion || seenVersions.has(relVersion)) continue;

    const ipa = findIosIpaAsset(rel.assets ?? []);
    if (!ipa || !Number.isFinite(ipa.size) || ipa.size <= 0) continue;

    seenVersions.add(relVersion);
    versions.push({
      version: relVersion,
      date: rel.published_at || rel.created_at || pubDate,
      localizedDescription: cleanNotes(rel.body ?? '', relVersion),
      downloadURL: downloadUrl(rel.tag_name, ipa.name),
      size: ipa.size,
      minOSVersion: '15.0',
    });
  }

  const latest = versions[0];
  const iconURL = `https://raw.githubusercontent.com/${repo}/main/apps/ios/src-tauri/icons/ios/AppIcon-512%402x.png`;

  return {
    name: 'Groovie',
    identifier: `${bundleIdentifier}.source`,
    subtitle: 'Local LAN control for Govee lights',
    description:
      'Official SideStore & AltStore source for Groovie — fast, cloud-free Govee light control over local Wi-Fi.',
    iconURL,
    website: `https://github.com/${repo}`,
    tintColor: '#FFB020',
    featuredApps: [bundleIdentifier],
    apps: [
      {
        name: 'Groovie',
        bundleIdentifier,
        developerName: 'Rudy Orre',
        subtitle: 'Local LAN control for Govee lights',
        localizedDescription:
          'Discover and control your Govee lights directly over local Wi-Fi UDP with zero cloud accounts, custom groups, one-tap scenes, and dotfiles-friendly groovie.json sync.',
        iconURL,
        tintColor: '#FFB020',
        category: 'utilities',
        // Legacy top-level version fields kept alongside `versions` for older SideStore parsers.
        version: latest.version,
        versionDate: latest.date,
        versionDescription: latest.localizedDescription,
        downloadURL: latest.downloadURL,
        size: latest.size,
        versions,
        appPermissions: {
          entitlements: [],
          privacy: {
            NSLocalNetworkUsageDescription: LOCAL_NETWORK_REASON,
          },
        },
      },
    ],
    news: [],
    userInfo: {},
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
  const outPath = process.env.ALTSTORE_OUTPUT_PATH || 'altstore-source.json';

  if (!repo || !tag || !releaseId) {
    throw new Error('Expected GITHUB_REPOSITORY, TAG, and RELEASE_ID environment variables');
  }

  const tauriConf = JSON.parse(readFileSync('apps/ios/src-tauri/tauri.conf.json', 'utf8'));
  const release = JSON.parse(ghApi([`repos/${repo}/releases/${releaseId}`]));
  const currentAssets = JSON.parse(ghApi(['--paginate', `repos/${repo}/releases/${releaseId}/assets`]));
  const previousReleases = JSON.parse(ghApi([`repos/${repo}/releases?per_page=25`]));

  const source = buildAltStoreSource({
    version: tauriConf.version,
    bundleIdentifier: tauriConf.identifier,
    tag,
    repo,
    notes: release.body || '',
    pubDate: release.published_at || release.created_at || new Date().toISOString(),
    currentAssets,
    previousReleases,
  });

  writeFileSync(outPath, JSON.stringify(source, null, 2) + '\n');
  console.log(
    `Wrote ${outPath} for ${tag} (${source.apps[0].versions.length} iOS release version(s)).`
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
