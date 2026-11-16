import assert from 'node:assert/strict';
import test from 'node:test';
import { buildLatestManifest } from './generate-latest-json.mjs';

function fullAssets() {
  return [
    { name: 'grooviectl-windows-x64.zip' },
    { name: 'grooviectl-windows-arm64.zip' },
    { name: 'grooviectl-macos-universal.tar.gz' },
    { name: 'grooviectl-linux-x64.tar.gz' },
    { name: 'grooviectl-linux-arm64.tar.gz' },
    { name: 'Groovie_0.5.2_amd64.AppImage' },
    { name: 'Groovie_0.5.2_amd64.AppImage.sig' },
    { name: 'Groovie_0.5.2_amd64.deb' },
    { name: 'Groovie_0.5.2_amd64.deb.sig' },
    { name: 'Groovie_0.5.2_x64-setup.exe' },
    { name: 'Groovie_0.5.2_x64-setup.exe.sig' },
    { name: 'Groovie_universal.app.tar.gz' },
    { name: 'Groovie_universal.app.tar.gz.sig' },
  ];
}

function fullSignatures() {
  return new Map([
    ['Groovie_0.5.2_amd64.AppImage.sig', 'sig-linux-appimage'],
    ['Groovie_0.5.2_amd64.deb.sig', 'sig-linux-deb'],
    ['Groovie_0.5.2_x64-setup.exe.sig', 'sig-win-nsis'],
    ['Groovie_universal.app.tar.gz.sig', 'sig-mac-universal'],
  ]);
}

test('buildLatestManifest builds updater manifest with changelog notes and all CLI archives', () => {
  const notes = "Download the installer below.\n\n### What's Changed since v0.5.1\n- feat: polish UI (abc1234)";
  const manifest = buildLatestManifest({
    version: '0.5.2',
    tag: 'v0.5.2',
    repo: 'rudyorre/groovie',
    notes,
    pubDate: '2026-11-16T00:00:00.000Z',
    assets: fullAssets(),
    signatures: fullSignatures(),
  });

  assert.equal(manifest.version, '0.5.2');
  assert.equal(manifest.notes, notes);
  assert.equal(manifest.pub_date, '2026-11-16T00:00:00.000Z');
  assert.equal(manifest.platforms['windows-x86_64'].signature, 'sig-win-nsis');
  assert.equal(manifest.platforms['darwin-aarch64'].signature, 'sig-mac-universal');
  assert.equal(manifest.platforms['linux-x86_64-appimage'].signature, 'sig-linux-appimage');
  assert.equal(manifest.platforms['linux-x86_64-deb'].signature, 'sig-linux-deb');
});

test('buildLatestManifest rejects missing ARM64 CLI archives or tag mismatch', () => {
  assert.throws(
    () =>
      buildLatestManifest({
        version: '0.5.2',
        tag: 'v0.5.2',
        repo: 'rudyorre/groovie',
        notes: '',
        assets: fullAssets().filter((a) => a.name !== 'grooviectl-windows-arm64.zip'),
        signatures: fullSignatures(),
      }),
    /Missing required CLI release asset: grooviectl-windows-arm64\.zip/,
  );

  assert.throws(
    () =>
      buildLatestManifest({
        version: '0.5.2',
        tag: 'v0.5.1',
        repo: 'rudyorre/groovie',
        notes: '',
        assets: fullAssets(),
        signatures: fullSignatures(),
      }),
    /does not match tauri\.conf\.json version/,
  );
});
