import assert from 'node:assert/strict';
import test from 'node:test';
import { buildAltStoreSource } from './generate-altstore-source.mjs';

test('buildAltStoreSource builds SideStore & AltStore compatible manifest with version history', () => {
  const source = buildAltStoreSource({
    version: '0.3.0',
    bundleIdentifier: 'dev.rudyorre.groovie',
    tag: 'v0.3.0',
    repo: 'rudyorre/groovie',
    notes: 'Added free iOS sideload support.',
    pubDate: '2026-11-15T20:00:00Z',
    currentAssets: [
      { name: 'grooviectl-linux-x64.tar.gz', size: 1200000 },
      { name: 'Groovie_0.3.0_ios.ipa', size: 6543210 },
    ],
    previousReleases: [
      // Drafts and desktop-only older releases should be skipped cleanly.
      {
        tag_name: 'v0.3.1-draft',
        draft: true,
        assets: [{ name: 'Groovie_0.3.1_ios.ipa', size: 7000000 }],
      },
      {
        tag_name: 'v0.2.9',
        draft: false,
        prerelease: false,
        published_at: '2026-10-01T12:00:00Z',
        body: 'Desktop-only release',
        assets: [{ name: 'Groovie_0.2.9_x64-setup.exe', size: 4100000 }],
      },
      {
        tag_name: 'v0.2.8',
        draft: false,
        prerelease: false,
        published_at: '2026-09-20T09:30:00Z',
        body: 'Earlier iOS build',
        assets: [{ name: 'Groovie_0.2.8_ios.ipa', size: 6123456 }],
      },
    ],
  });

  assert.equal(source.name, 'Groovie');
  assert.equal(source.identifier, 'dev.rudyorre.groovie.source');
  assert.equal(source.apps.length, 1);

  const app = source.apps[0];
  assert.equal(app.bundleIdentifier, 'dev.rudyorre.groovie');
  assert.equal(app.version, '0.3.0');
  assert.equal(app.size, 6543210);
  assert.equal(
    app.downloadURL,
    'https://github.com/rudyorre/groovie/releases/download/v0.3.0/Groovie_0.3.0_ios.ipa'
  );
  assert.equal(app.versions.length, 2);
  assert.deepEqual(
    app.versions.map((v) => v.version),
    ['0.3.0', '0.2.8']
  );
  assert.ok(app.appPermissions.privacy.NSLocalNetworkUsageDescription.includes('Wi-Fi'));
});

test('buildAltStoreSource throws if iOS IPA asset is missing or tag mismatches', () => {
  assert.throws(
    () =>
      buildAltStoreSource({
        version: '0.3.0',
        bundleIdentifier: 'dev.rudyorre.groovie',
        tag: 'v0.3.1',
        repo: 'rudyorre/groovie',
        notes: '',
        currentAssets: [{ name: 'Groovie_0.3.0_ios.ipa', size: 1024 }],
      }),
    /does not match/
  );

  assert.throws(
    () =>
      buildAltStoreSource({
        version: '0.3.0',
        bundleIdentifier: 'dev.rudyorre.groovie',
        tag: 'v0.3.0',
        repo: 'rudyorre/groovie',
        notes: '',
        currentAssets: [{ name: 'Groovie_0.3.0_amd64.deb', size: 1024 }],
      }),
    /Missing required iOS \.ipa asset/
  );
});
