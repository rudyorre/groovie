<script lang="ts">
  import { onMount, tick } from 'svelte';
  import {
    api,
    isTauri,
    toHex,
    type ConfigInfo,
    type DesktopPrefs,
    type Device,
    type Group,
    type LogEntry,
    type ReleaseVersionInfo,
    type Scene,
    type SyncConfig,
    type SyncPreview,
    type SyncState,
    type Target,
    type Update,
    type UpdateProgress,
  } from './lib/api';
  import DeviceCard from './lib/DeviceCard.svelte';
  import Icon from './lib/Icon.svelte';
  import MasterControlBar from './lib/MasterControlBar.svelte';
  import ScreenSyncView from './lib/ScreenSyncView.svelte';
  import { applyTheme, readTheme, type ThemeMode } from './lib/theme';

  type SceneScope = 'all' | 'group' | 'custom';
  type SceneModalState = {
    id: string;
    name: string;
    scope: SceneScope;
    groupId: string;
    members: string[];
  };

  let devices = $state<Device[]>([]);
  let groups = $state<Group[]>([]);
  let scenes = $state<Scene[]>([]);
  let syncState = $state<SyncState | null>(null);
  let syncPreview = $state<SyncPreview | null>(null);
  /** 'all', 'sync', or a group id */
  let view = $state('all');
  let toast = $state('');
  let toastAction = $state<{ label: string; run: () => void } | null>(null);
  let startupError = $state('');
  let version = $state('');
  let update = $state<Update | null>(null);
  let updateProgress = $state<UpdateProgress | null>(null);

  // Dialogs / inline editors
  let editing = $state<Group | null>(null); // id '' = new group
  let sceneModal = $state<SceneModalState | null>(null);
  let addingLight = $state(false);
  let ipDraft = $state('');
  let scanning = $state(false);
  let logsOpen = $state(false);
  let logs = $state<LogEntry[]>([]);
  let hidePolls = $state(true);
  let copied = $state(false);
  let logBox = $state<HTMLElement | null>(null);
  let configOpen = $state(false);
  let configInfo = $state<ConfigInfo | null>(null);
  let desktopPrefs = $state<DesktopPrefs>({ launchAtLogin: false, startMinimized: false });
  let theme = $state<ThemeMode>(readTheme());
  let editingPath = $state(false);
  let pathDraft = $state('');
  let copiedPath = $state(false);
  let copiedConfig = $state(false);
  let editingPin = $state(false);
  let releases = $state<ReleaseVersionInfo[]>([]);
  let loadingReleases = $state(false);
  let releasesError = $state('');
  let pinChoice = $state('');
  let customPinDraft = $state('');

  function selectTheme(next: ThemeMode) {
    theme = next;
    applyTheme(next);
  }

  function modalFocusTrap(node: HTMLElement) {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const selector =
      'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

    queueMicrotask(() => {
      if (!node.contains(document.activeElement)) {
        node.querySelector<HTMLElement>(selector)?.focus();
      }
    });

    function onKeydown(e: KeyboardEvent) {
      if (e.key !== 'Tab') return;
      const focusables = Array.from(node.querySelectorAll<HTMLElement>(selector)).filter(
        (el) => el.offsetParent !== null || el === document.activeElement,
      );
      if (focusables.length === 0) {
        e.preventDefault();
        return;
      }
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (e.shiftKey) {
        if (!active || !node.contains(active) || active === first) {
          e.preventDefault();
          last.focus();
        }
      } else if (!active || !node.contains(active) || active === last) {
        e.preventDefault();
        first.focus();
      }
    }

    node.addEventListener('keydown', onKeydown);
    return {
      destroy() {
        node.removeEventListener('keydown', onKeydown);
        if (opener && opener.isConnected) {
          opener.focus();
        }
      },
    };
  }

  const group = $derived(groups.find((g) => g.id === view));
  const shown = $derived(group ? devices.filter((d) => group.members.includes(d.id)) : devices);
  const target = $derived<Target>(group ? { type: 'group', id: group.id } : { type: 'all' });
  const isInvalidConfigError = $derived(startupError.startsWith('Invalid config file'));
  const peerConflict = $derived(syncState && !syncState.active ? (syncState.conflict ?? null) : null);
  const peerSyncedIds = $derived(peerConflict?.deviceIds ?? []);
  const peerSyncedSet = $derived(new Set(peerSyncedIds));
  const peerConflictOverlapsIncluded = $derived(
    Boolean(
      peerConflict &&
        syncState?.config.placements.some((p) => !p.excluded && peerSyncedSet.has(p.deviceId)),
    ),
  );
  const controllableShown = $derived(shown.filter((d) => !peerSyncedSet.has(d.id)));
  const onCount = $derived(controllableShown.filter((d) => d.online && d.state?.on).length);
  const shownLogs = $derived(hidePolls ? logs.filter((l) => !l.poll) : logs);
  function parseSemverTuple(raw?: string | null): [number, number, number] | null {
    if (!raw) return null;
    const m = /^[vV]?(\d+)\.(\d+)\.(\d+)$/.exec(raw.trim());
    if (!m) return null;
    return [Number(m[1]), Number(m[2]), Number(m[3])];
  }

  function cmpSemver(a: [number, number, number], b: [number, number, number]): number {
    return a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
  }

  const minPinnable = $derived(configInfo?.minPinnableVersion ?? '0.5.3');
  const versionChoices = $derived.by(() => {
    const floor = parseSemverTuple(minPinnable) ?? [0, 5, 3];
    const map = new Map<string, ReleaseVersionInfo>();
    for (const r of releases) {
      map.set(r.version, r);
    }
    for (const extra of [version, configInfo?.pinnedVersion]) {
      const tup = parseSemverTuple(extra);
      if (extra && tup && cmpSemver(tup, floor) >= 0) {
        const clean = `${tup[0]}.${tup[1]}.${tup[2]}`;
        if (!map.has(clean)) {
          map.set(clean, { version: clean, isLatest: false });
        }
      }
    }
    return Array.from(map.values()).sort((x, y) => {
      const tx = parseSemverTuple(x.version) ?? [0, 0, 0];
      const ty = parseSemverTuple(y.version) ?? [0, 0, 0];
      return cmpSemver(ty, tx);
    });
  });
  const selectedPinTarget = $derived(
    (pinChoice === 'custom' ? customPinDraft : pinChoice).trim().replace(/^[vV]/, ''),
  );
  const updating = $derived(updateProgress !== null);
  const updatePct = $derived(
    updateProgress?.stage === 'downloading' ? (updateProgress.percent ?? 15) : updateProgress ? 100 : 0,
  );
  const updateLabel = $derived(
    !updateProgress
      ? update?.pinned
        ? `Switch to v${update.version}`
        : `Update to ${update?.version ?? ''}`
      : updateProgress.stage === 'downloading'
        ? updateProgress.percent !== null
          ? `Downloading ${updateProgress.percent}%`
          : 'Downloading…'
        : updateProgress.stage === 'installing'
          ? 'Installing…'
          : 'Restarting…',
  );

  let toastTimer: ReturnType<typeof setTimeout>;
  function showToast(msg: string, action?: { label: string; run: () => void }, durationMs = 4000) {
    toast = msg;
    toastAction = action ?? null;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      toast = '';
      toastAction = null;
    }, durationMs);
  }

  async function run<T>(f: (a: Awaited<typeof api>) => Promise<T>) {
    try {
      return await f(await api);
    } catch (e) {
      showToast(String(e));
    }
  }

  let lastUpdateCheck = 0;
  async function refreshUpdate(force = false) {
    if (updating) return;
    if (!force && (update || Date.now() - lastUpdateCheck < 60_000)) return;
    lastUpdateCheck = Date.now();
    const a = await api;
    update = await a.checkUpdate().catch(() => null);
  }

  async function reload() {
    try {
      const a = await api;
      [devices, groups, scenes, syncState] = await Promise.all([
        a.listDevices(),
        a.listGroups(),
        a.listScenes(),
        a.getSyncState(),
      ]);
      startupError = '';
      if (view === 'sync' && !syncState.supported) view = 'all';
      else if (view !== 'all' && view !== 'sync' && !groups.some((g) => g.id === view)) view = 'all';
    } catch (e) {
      startupError = String(e);
    }
  }

  async function toggleSync(enabled: boolean, force = false) {
    const next = await run((a) => a.setSyncEnabled(enabled, force));
    if (next) {
      syncState = next;
      if (!next.active) syncPreview = null;
    } else if (enabled && !force) {
      const refreshed = await (await api).getSyncState().catch(() => null);
      if (refreshed) syncState = refreshed;
    }
  }

  async function updateSyncCfg(nextCfg: SyncConfig) {
    const next = await run((a) => a.updateSyncConfig(nextCfg));
    if (next) syncState = next;
  }

  async function identifyPlacement(placementId: string) {
    await run((a) => a.identifySyncPlacement(placementId));
  }

  onMount(() => {
    reload();
    let off = () => {};
    const onFocus = () => {
      refreshUpdate(false);
      api.then((a) => a.setWindowActive(true, true)).catch(() => {});
    };
    const onVisibility = () => {
      const visible = !document.hidden;
      api.then((a) => a.setWindowActive(visible, visible)).catch(() => {});
    };
    window.addEventListener('focus', onFocus);
    document.addEventListener('visibilitychange', onVisibility);
    api.then(async (a) => {
      off = a.onEvent((e) => {
        if (e.type === 'deviceUpdated') {
          const i = devices.findIndex((d) => d.id === e.device.id);
          if (i >= 0) devices[i] = e.device;
          else devices = [...devices, e.device].sort((x, y) => x.name.localeCompare(y.name));
        } else if (e.type === 'syncChanged') {
          syncState = { supported: syncState?.supported ?? false, ...e.status };
          if (!e.status.active) syncPreview = null;
        } else if (e.type === 'syncPreview') {
          syncPreview = e.preview;
        } else if (e.type === 'configChanged') {
          reload();
          a.getConfigInfo()
            .then((info) => {
              configInfo = info;
              if (!editingPath) pathDraft = info.path ?? '';
            })
            .catch(() => {});
          refreshUpdate(true);
        } else if (e.type === 'logged' && logsOpen) {
          logs = [...logs.slice(-499), e.entry];
        }
      });
      await a.setWindowActive(!document.hidden, false).catch(() => {});
      const [ver, info] = await Promise.all([
        a.appVersion().catch(() => ''),
        a.getConfigInfo().catch(() => null),
      ]);
      version = ver;
      if (info) configInfo = info;
      await refreshUpdate(true);
    });
    return () => {
      off();
      window.removeEventListener('focus', onFocus);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  });

  async function rescan() {
    scanning = true;
    await run((a) => a.discover());
    if (startupError) await reload();
    setTimeout(() => (scanning = false), 1200);
  }

  async function openLogs() {
    logsOpen = true;
    copied = false;
    const fetched = await run((a) => a.listLogs());
    if (fetched) logs = fetched;
    await tick();
    if (logBox) logBox.scrollTop = logBox.scrollHeight;
  }

  async function clearLogEntries() {
    await run((a) => a.clearLogs());
    logs = [];
  }

  async function openConfig() {
    configOpen = true;
    editingPath = false;
    editingPin = false;
    copiedPath = false;
    copiedConfig = false;
    const [info, prefs] = await Promise.all([
      run((a) => a.getConfigInfo()),
      run((a) => a.getDesktopPrefs()),
    ]);
    if (info) {
      configInfo = info;
      pathDraft = info.path ?? '';
    }
    if (prefs) {
      desktopPrefs = prefs;
    }
  }

  async function togglePinEditor() {
    if (editingPin) {
      editingPin = false;
      return;
    }
    editingPin = true;
    releasesError = '';
    loadingReleases = true;
    try {
      const a = await api;
      releases = await a.listPinnableReleases();
    } catch (e) {
      releasesError = String(e);
    } finally {
      loadingReleases = false;
    }
    const preferred = configInfo?.pinnedVersion || version;
    if (preferred && versionChoices.some((c) => c.version === preferred)) {
      pinChoice = preferred;
      customPinDraft = '';
    } else if (versionChoices.length > 0) {
      pinChoice = versionChoices[0].version;
      customPinDraft = '';
    } else {
      pinChoice = 'custom';
      customPinDraft = preferred || minPinnable;
    }
  }

  function fmtReleaseDate(iso?: string): string {
    if (!iso) return '';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '';
    return d.toISOString().slice(0, 10);
  }

  async function applyPinVersion() {
    const targetVer = selectedPinTarget;
    if (!targetVer || updating) return;
    if (version && targetVer === version) {
      const info = await run((a) => a.setPinnedVersion(targetVer));
      if (info) {
        configInfo = info;
        update = null;
        editingPin = false;
        showToast(`Pinned Groovie to v${targetVer}`);
      }
      return;
    }
    updateProgress = { stage: 'downloading', percent: null };
    try {
      const a = await api;
      await a.installVersion(targetVer, true, (p) => {
        updateProgress = p;
      });
      if (!isTauri) {
        version = await a.appVersion();
        configInfo = await a.getConfigInfo();
        updateProgress = null;
        editingPin = false;
        showToast(`Pinned & switched to v${targetVer}`);
      }
    } catch (e) {
      updateProgress = null;
      showToast(`Could not switch to v${targetVer}: ${e}`);
    }
  }

  async function installConfiguredPin() {
    const targetVer = configInfo?.pinnedVersion;
    if (!targetVer || targetVer === version || updating) return;
    updateProgress = { stage: 'downloading', percent: null };
    try {
      const a = await api;
      await a.installVersion(targetVer, false, (p) => {
        updateProgress = p;
      });
      if (!isTauri) {
        version = await a.appVersion();
        updateProgress = null;
        showToast(`Switched to pinned v${targetVer}`);
      }
    } catch (e) {
      updateProgress = null;
      showToast(`Could not install v${targetVer}: ${e}`);
    }
  }

  async function unpinVersion() {
    if (updating) return;
    const info = await run((a) => a.setPinnedVersion(null));
    if (info) {
      configInfo = info;
      editingPin = false;
      await refreshUpdate(true);
      showToast('Unpinned version — update suggestions enabled');
    }
  }

  async function updateDesktopPref(key: keyof DesktopPrefs, value: boolean) {
    const prev = { ...desktopPrefs };
    const optimistic = { ...desktopPrefs, [key]: value };
    desktopPrefs = optimistic;
    const saved = await run((a) => a.setDesktopPrefs(optimistic));
    desktopPrefs = saved ?? prev;
  }

  async function copyConfigPath() {
    if (!configInfo?.path) return;
    try {
      await navigator.clipboard.writeText(configInfo.path);
      copiedPath = true;
      setTimeout(() => (copiedPath = false), 2000);
    } catch (e) {
      showToast(`Could not copy: ${e}`);
    }
  }

  async function copyConfigJson() {
    if (!configInfo) return;
    try {
      await navigator.clipboard.writeText(configInfo.json);
      copiedConfig = true;
      setTimeout(() => (copiedConfig = false), 2000);
    } catch (e) {
      showToast(`Could not copy: ${e}`);
    }
  }

  async function reloadConfigFromDisk() {
    const info = await run((a) => a.reloadConfig());
    if (info) {
      configInfo = info;
      pathDraft = info.path ?? '';
      await reload();
      await refreshUpdate(true);
      showToast('Reloaded configuration from disk');
    }
  }

  async function saveCustomPath() {
    const next = pathDraft.trim();
    if (!next) return;
    const info = await run((a) => a.setConfigPath(next));
    if (info) {
      configInfo = info;
      pathDraft = info.path ?? '';
      editingPath = false;
      await reload();
      await refreshUpdate(true);
      showToast('Updated config file location');
    }
  }

  async function resetConfigPath() {
    const info = await run((a) => a.setConfigPath(null));
    if (info) {
      configInfo = info;
      pathDraft = info.path ?? '';
      editingPath = false;
      await reload();
      await refreshUpdate(true);
      showToast('Restored default config location');
    }
  }

  function fmtTime(tsMs: number) {
    const d = new Date(tsMs);
    const p2 = (n: number) => String(n).padStart(2, '0');
    const p3 = (n: number) => String(n).padStart(3, '0');
    return `${p2(d.getHours())}:${p2(d.getMinutes())}:${p2(d.getSeconds())}.${p3(d.getMilliseconds())}`;
  }

  function dirTag(dir: string) {
    if (dir === 'action') return 'ACT';
    if (dir === 'state') return 'CHG';
    return dir.toUpperCase();
  }

  function fmtDeviceSummary(d: Device) {
    if (!d.online) return 'offline';
    if (!d.state) return 'online';
    const { color, kelvin } = d.state;
    const mode = kelvin
      ? `${kelvin}K`
      : color.r === 0 && color.g === 0 && color.b === 0
        ? 'scene'
        : toHex(color);
    return `${d.state.on ? 'on' : 'off'} · ${d.state.brightness}% · ${mode}`;
  }

  async function copyLogs() {
    const fmtLine = (l: LogEntry) => {
      const who = [l.device, l.addr].filter(Boolean).join(' @ ');
      return `${fmtTime(l.tsMs)} [${dirTag(l.dir)}]${who ? ` ${who}:` : ''} ${l.message}`;
    };
    const devLines = devices.map(
      (d) => `- ${d.name} (${d.sku}) | ip=${d.ip} | id=${d.id} | ${fmtDeviceSummary(d)}${d.firmware ? ` | ${d.firmware}` : ''}`,
    );
    const events = logs.filter((l) => !l.poll).map(fmtLine);
    const recentRaw = logs.slice(-80).map(fmtLine);
    const report = [
      `Groovie ${version ? `v${version}` : 'demo'} · ${new Date().toISOString()}`,
      `Devices (${devices.length}):`,
      ...(devLines.length ? devLines : ['  (none)']),
      ...(groups.length ? [`Groups: ${groups.map((g) => `${g.name}=[${g.members.join(',')}]`).join('; ')}`] : []),
      '',
      `Events (${events.length}):`,
      ...(events.length ? events : ['  (none)']),
      '',
      `Recent packets (${recentRaw.length}):`,
      ...(recentRaw.length ? recentRaw : ['  (none)']),
    ].join('\n');
    try {
      await navigator.clipboard.writeText(report);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch (e) {
      showToast(`Could not copy: ${e}`);
    }
  }

  async function saveGroup() {
    const g = editing!;
    editing = null;
    if (!g.name.trim()) return;
    if (g.id) await run((a) => a.updateGroup({ ...g, name: g.name.trim() }));
    else {
      const created = await run((a) => a.createGroup(g.name.trim(), g.members));
      if (created) view = created.id;
    }
  }

  function openCreateScene() {
    if (group && group.members.length > 0) {
      sceneModal = {
        id: '',
        name: '',
        scope: 'group',
        groupId: group.id,
        members: [...group.members],
      };
    } else {
      sceneModal = {
        id: '',
        name: '',
        scope: 'all',
        groupId: groups[0]?.id ?? '',
        members: devices.map((d) => d.id),
      };
    }
  }

  function openEditScene(sc: Scene) {
    const entryIds = sc.entries.map((e) => e.device);
    const matchingGroup = groups.find(
      (g) =>
        g.members.length > 0 &&
        g.members.length === entryIds.length &&
        g.members.every((m) => entryIds.includes(m)),
    );
    const isAll =
      devices.length > 0 &&
      entryIds.length === devices.length &&
      devices.every((d) => entryIds.includes(d.id));
    sceneModal = {
      id: sc.id,
      name: sc.name,
      scope: matchingGroup ? 'group' : isAll ? 'all' : 'custom',
      groupId: matchingGroup?.id ?? group?.id ?? groups[0]?.id ?? '',
      members: [...entryIds],
    };
  }

  function resolveSceneModalDeviceIds(modal: SceneModalState): string[] | undefined {
    if (modal.scope === 'all') return undefined;
    if (modal.scope === 'group') {
      const g = groups.find((x) => x.id === modal.groupId);
      return g ? [...g.members] : [];
    }
    return [...modal.members];
  }

  async function submitSceneModal() {
    if (!sceneModal) return;
    const modal = $state.snapshot(sceneModal);
    const name = modal.name.trim();
    if (!name) return;
    const devIds = resolveSceneModalDeviceIds(modal);
    if (devIds && devIds.length === 0) {
      showToast('Choose at least one light for this scene');
      return;
    }
    sceneModal = null;

    if (!modal.id) {
      const saved = await run((a) => a.saveScene(name, devIds));
      if (saved) {
        showToast(
          `Saved scene "${saved.name}" (${saved.entries.length} ${saved.entries.length === 1 ? 'light' : 'lights'})`,
        );
      }
      return;
    }

    const existing = scenes.find((s) => s.id === modal.id);
    if (!existing) return;
    const targetIds = devIds ?? devices.map((d) => d.id);
    const prevIds = existing.entries.map((e) => e.device);
    const sameScope = targetIds.length === prevIds.length && targetIds.every((id) => prevIds.includes(id));

    if (sameScope) {
      if (name !== existing.name) {
        await run((a) => a.renameScene(modal.id, name));
      }
    } else {
      const nextEntries = targetIds.flatMap((id) => {
        const prev = existing.entries.find((e) => e.device === id);
        if (prev) return [structuredClone($state.snapshot(prev))];
        const d = devices.find((x) => x.id === id);
        if (!d?.state) return [];
        const hasColor = d.state.color.r !== 0 || d.state.color.g !== 0 || d.state.color.b !== 0;
        return [
          {
            device: d.id,
            on: d.state.on,
            brightness: d.state.brightness,
            ...(d.state.kelvin
              ? { kelvin: d.state.kelvin }
              : { color: hasColor ? d.state.color : (d.lastColor ?? { r: 255, g: 180, b: 100 }) }),
          },
        ];
      });
      await run((a) => a.restoreScene({ id: modal.id, name, entries: nextEntries }));
    }
  }

  async function captureSceneCurrentLook() {
    if (!sceneModal || !sceneModal.id) return;
    const modal = $state.snapshot(sceneModal);
    const name = modal.name.trim();
    if (!name) return;
    const devIds = resolveSceneModalDeviceIds(modal);
    if (devIds && devIds.length === 0) {
      showToast('Choose at least one light for this scene');
      return;
    }
    sceneModal = null;
    const updated = await run((a) => a.updateScene(modal.id, name, devIds));
    if (updated) {
      showToast(
        `Updated "${updated.name}" (${updated.entries.length} ${updated.entries.length === 1 ? 'light' : 'lights'})`,
      );
    }
  }

  async function deleteSceneWithUndo(sc: Scene) {
    const backup = structuredClone($state.snapshot(sc));
    await run((a) => a.deleteScene(backup.id));
    showToast(
      `Deleted scene "${backup.name}"`,
      {
        label: 'Undo',
        run: () => {
          toast = '';
          toastAction = null;
          clearTimeout(toastTimer);
          run((a) => a.restoreScene(backup));
        },
      },
      6000,
    );
  }

  async function addLight() {
    const ip = ipDraft.trim();
    addingLight = false;
    ipDraft = '';
    if (ip) await run((a) => a.addIp(ip));
  }

  async function forgetDevice(d: Device) {
    const ok = await run(async (a) => {
      await a.removeDevice(d.id);
      return true;
    });
    if (ok) {
      showToast(`Forgot "${d.name}"`);
    }
  }

  async function installUpdate() {
    if (!update || updating) return;
    updateProgress = { stage: 'downloading', percent: null };
    try {
      await update.install((p) => {
        updateProgress = p;
      });
    } catch (e) {
      updateProgress = null;
      showToast(`Update failed: ${e}`);
    }
  }
</script>

<div class="app">
  <aside>
    <div class="brand">
      <svg viewBox="0 0 1024 1024" aria-hidden="true">
        <path d="M768 466.9A260 260 0 1 1 557.1 255.9" fill="none" stroke="currentColor" stroke-width="120" stroke-linecap="round" />
        <circle cx="695.8" cy="328.2" r="60" fill="#FFB020" />
      </svg>
      Groovie
    </div>

    <nav>
      <button class="nav-item" class:active={view === 'all'} onclick={() => (view = 'all')}>
        <Icon name="grid" /> <span>All lights</span> <small>{devices.length}</small>
      </button>

      {#if syncState?.supported}
        <div
          class="sync-nav"
          class:active={view === 'sync'}
          class:live={syncState.active || peerConflictOverlapsIncluded}
        >
          <button class="nav-item" onclick={() => (view = 'sync')}>
            <Icon name="monitor" /> <span>Screen sync</span>
          </button>
          <label
            class="switch nav-switch"
            title={syncState.active
              ? 'Stop screen sync'
              : peerConflictOverlapsIncluded
                ? `Active on ${peerConflict?.hostName} — open Screen Sync to take over`
                : 'Start screen sync'}
          >
            <input
              type="checkbox"
              checked={syncState.active}
              disabled={!syncState.active && peerConflictOverlapsIncluded}
              onchange={(e) => toggleSync(e.currentTarget.checked)}
            />
            <span></span>
          </label>
        </div>
      {/if}

      <div class="section">
        <span>Groups</span>
        <button class="ghost icon-btn" title="New group" onclick={() => (editing = { id: '', name: '', members: [] })}>
          <Icon name="plus" size={14} />
        </button>
      </div>
      {#each groups as g (g.id)}
        <button class="nav-item" class:active={view === g.id} onclick={() => (view = g.id)}>
          <Icon name="folder" /> <span>{g.name}</span> <small>{g.members.length}</small>
        </button>
      {:else}
        <p class="empty-hint">Group lights to control them together.</p>
      {/each}

      <div class="section">
        <span>Scenes</span>
        <button class="ghost icon-btn" title="Save current look as a scene" onclick={openCreateScene}>
          <Icon name="plus" size={14} />
        </button>
      </div>
      {#each scenes as sc (sc.id)}
        <div class="scene">
          <button class="nav-item" title="Apply scene ({sc.entries.length} {sc.entries.length === 1 ? 'light' : 'lights'})" onclick={() => run((a) => a.applyScene(sc.id))}>
            <Icon name="sparkle" /> <span>{sc.name}</span> <small class="scene-count">{sc.entries.length}</small>
          </button>
          <div class="scene-actions">
            <button class="ghost icon-btn" title="Edit scene" onclick={() => openEditScene(sc)}>
              <Icon name="pencil" size={13} />
            </button>
            <button class="ghost icon-btn" title="Delete scene" onclick={() => deleteSceneWithUndo(sc)}>
              <Icon name="x" size={14} />
            </button>
          </div>
        </div>
      {:else}
        <p class="empty-hint">Save the current look with +.</p>
      {/each}
    </nav>

    <footer>
      {#if update}
        <button
          class="update"
          class:busy={updating}
          class:spin={updateProgress !== null && updateProgress.stage !== 'downloading'}
          style:--pct="{updatePct}%"
          title={update.pinned
            ? `Pinned to v${update.version} in groovie.json — install and restart`
            : update.notes
              ? `Groovie v${update.version}\n\n${update.notes}`
              : `Install Groovie v${update.version}`}
          onclick={installUpdate}
          disabled={updating}
        >
          <Icon name={updateProgress && updateProgress.stage !== 'downloading' ? 'refresh' : 'download'} size={14} />
          <span>{updateLabel}</span>
        </button>
      {/if}
      <div class="foot-row">
        <div class="foot-left">
          <button
            class="ghost version-link"
            title="View Groovie on GitHub"
            onclick={() => run((a) => a.openGithub())}
          >
            <span>{isTauri ? `v${version}` : `Demo v${version}`}</span>
            <Icon name="external" size={11} />
          </button>
          {#if configInfo?.pinnedVersion}
            <button
              class="ghost pin-pill"
              title="Pinned to v{configInfo.pinnedVersion} — open Configuration to change or unpin"
              onclick={openConfig}
            >
              <Icon name="pin" size={11} />
              <span>Pinned</span>
            </button>
          {/if}
        </div>
        <div class="foot-tools">
          <button
            class="ghost foot-icon"
            title="Configuration (groovie.json)"
            aria-label="Configuration"
            onclick={openConfig}
          >
            <Icon name="sliders" size={14} />
          </button>
          <button
            class="ghost foot-icon"
            title="Activity & network log"
            aria-label="Activity & network log"
            onclick={openLogs}
          >
            <Icon name="terminal" size={14} />
          </button>
        </div>
      </div>
    </footer>
  </aside>

  <main>
    {#if view === 'sync' && syncState?.supported}
      {#if startupError}
        <div class="banner" role="alert">
          <div>
            <strong>{isInvalidConfigError ? 'Configuration file has invalid JSON' : 'Could not start local network listener'}</strong>
            <p>{startupError}</p>
          </div>
          <div class="banner-actions">
            {#if isInvalidConfigError}
              <button class="ghost" onclick={openConfig}><Icon name="sliders" size={14} /> Open config</button>
            {/if}
            <button onclick={rescan}><Icon name="refresh" size={14} /> Retry</button>
          </div>
        </div>
      {/if}
      <ScreenSyncView
        {devices}
        {syncState}
        preview={syncPreview}
        onToggleSync={toggleSync}
        onUpdateConfig={updateSyncCfg}
        onIdentify={identifyPlacement}
      />
    {:else}
      <header>
        <div>
          <h1>{group?.name ?? 'All lights'}</h1>
          <p class="sub">
            {shown.length} {shown.length === 1 ? 'light' : 'lights'}{#if shown.length}{' · '}{onCount} on{/if}{#if peerConflict && shown.some((d) => peerSyncedSet.has(d.id))}{' · '}synced by {peerConflict.hostName}{/if}
          </p>
        </div>
        <div class="actions">
          {#if group}
            <button class="ghost" onclick={() => (editing = structuredClone($state.snapshot(group)))}>
              <Icon name="pencil" size={14} /> Edit group
            </button>
          {/if}
          {#if shown.length > 0}
            <button class="ghost" title="Save current look as a scene" onclick={openCreateScene}>
              <Icon name="sparkle" size={14} /> Save scene
            </button>
          {/if}
          <button class="ghost icon-btn" title="Add light by IP" onclick={() => (addingLight = true)}><Icon name="plus" /></button>
          <button class="ghost icon-btn" class:spin={scanning} title="Search for lights" onclick={rescan}><Icon name="refresh" /></button>
          <button
            onclick={() => run((a) => a.apply(target, { type: 'power', value: onCount === 0 }))}
            disabled={!controllableShown.length}
          >
            <Icon name="power" size={14} /> {onCount ? 'All off' : 'All on'}
          </button>
        </div>
      </header>

      {#if startupError}
        <div class="banner" role="alert">
          <div>
            <strong>{isInvalidConfigError ? 'Configuration file has invalid JSON' : 'Could not start local network listener'}</strong>
            <p>{startupError}</p>
          </div>
          <div class="banner-actions">
            {#if isInvalidConfigError}
              <button class="ghost" onclick={openConfig}><Icon name="sliders" size={14} /> Open config</button>
            {/if}
            <button onclick={rescan}><Icon name="refresh" size={14} /> Retry</button>
          </div>
        </div>
      {/if}

      {#if group ? shown.length > 0 : shown.length > 1}
        <MasterControlBar
          {target}
          devices={shown}
          groupName={group?.name}
          {peerSyncedIds}
          peerSyncHost={peerConflict?.hostName ?? null}
        />
      {/if}

      <section class="grid">
        {#each shown as d (d.id)}
          <DeviceCard
            device={d}
            syncing={Boolean(syncState?.active && syncState.activeDevices.includes(d.id))}
            peerSyncHost={peerSyncedSet.has(d.id) ? (peerConflict?.hostName ?? null) : null}
            onOpenSync={syncState?.supported ? () => (view = 'sync') : undefined}
            onForget={forgetDevice}
          />
        {:else}
          <div class="empty">
            {#if group}
              <p class="empty-title">This group is empty</p>
              <p>Use <b>Edit group</b> to choose which lights belong here.</p>
            {:else}
              <p class="empty-title">Looking for lights…</p>
              <p>Turn on <b>LAN Control</b> for each light in the Govee app (device settings), and make sure this
                computer is on the same Wi‑Fi. You can also add a light by its IP address with <b>+</b>.</p>
            {/if}
          </div>
        {/each}
      </section>
    {/if}
  </main>
</div>

{#if editing}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (editing = null)}>
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-label={editing.id ? 'Edit group' : 'New group'}
      use:modalFocusTrap
    >
      <form class="dialog-form" onsubmit={(e) => { e.preventDefault(); saveGroup(); }}>
        <h2>{editing.id ? 'Edit group' : 'New group'}</h2>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="field" bind:value={editing.name} placeholder="Name, e.g. Desk" autofocus />
        <div class="checks">
          {#each devices as d (d.id)}
            <label class="check">
              <input type="checkbox" value={d.id} bind:group={editing.members} />
              <span>{d.name}</span>
              <small>{d.sku}</small>
            </label>
          {/each}
        </div>
        <div class="dialog-actions">
          {#if editing.id}
            <button
              type="button"
              class="ghost danger"
              onclick={() => { const id = editing!.id; editing = null; run((a) => a.deleteGroup(id)); }}
            ><Icon name="trash" size={14} /> Delete</button>
          {/if}
          <span class="spacer"></span>
          <button type="button" class="ghost" onclick={() => (editing = null)}>Cancel</button>
          <button type="submit" class="primary">Save</button>
        </div>
      </form>
    </div>
  </div>
{/if}

{#if sceneModal}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (sceneModal = null)}>
    <div
      class="dialog scene-dialog"
      role="dialog"
      aria-modal="true"
      aria-label={sceneModal.id ? 'Edit scene' : 'Save scene'}
      use:modalFocusTrap
    >
      <form class="dialog-form" onsubmit={(e) => { e.preventDefault(); submitSceneModal(); }}>
        <h2>{sceneModal.id ? 'Edit scene' : 'Save scene'}</h2>
        <p class="dialog-text">
          {sceneModal.id
            ? 'Rename this scene, adjust which lights it controls, or overwrite it with your current room look.'
            : 'Snapshot the current power, brightness, colour, or white temperature of the selected lights.'}
        </p>

        <!-- svelte-ignore a11y_autofocus -->
        <input class="field" bind:value={sceneModal.name} placeholder="Name, e.g. Late night" autofocus />

        <div class="scope-block">
          <span class="path-label">Lights in scene</span>
          <div class="segmented" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={sceneModal.scope === 'all'}
              class:sel={sceneModal.scope === 'all'}
              onclick={() => (sceneModal!.scope = 'all')}
            >
              All lights
            </button>
            {#if groups.length > 0}
              <button
                type="button"
                role="tab"
                aria-selected={sceneModal.scope === 'group'}
                class:sel={sceneModal.scope === 'group'}
                onclick={() => {
                  if (!sceneModal!.groupId && groups[0]) sceneModal!.groupId = groups[0].id;
                  sceneModal!.scope = 'group';
                }}
              >
                Group
              </button>
            {/if}
            <button
              type="button"
              role="tab"
              aria-selected={sceneModal.scope === 'custom'}
              class:sel={sceneModal.scope === 'custom'}
              onclick={() => (sceneModal!.scope = 'custom')}
            >
              Choose lights
            </button>
          </div>

          {#if sceneModal.scope === 'group'}
            <select class="field" bind:value={sceneModal.groupId} aria-label="Scene group">
              {#each groups as g (g.id)}
                <option value={g.id}>{g.name} ({g.members.length} {g.members.length === 1 ? 'light' : 'lights'})</option>
              {/each}
            </select>
          {:else if sceneModal.scope === 'custom'}
            <div class="checks">
              {#each devices as d (d.id)}
                <label class="check">
                  <input type="checkbox" value={d.id} bind:group={sceneModal.members} />
                  <span>{d.name}</span>
                  <small>{d.sku}</small>
                </label>
              {/each}
            </div>
          {:else}
            <p class="scope-hint">
              Includes {devices.length === 1 ? '1 discovered light' : `all ${devices.length} discovered lights`}.
            </p>
          {/if}
        </div>

        {#if sceneModal.id}
          <button
            type="button"
            class="capture-btn"
            disabled={!sceneModal.name.trim()}
            onclick={captureSceneCurrentLook}
          >
            <Icon name="sparkle" size={14} />
            <span>Overwrite with current look</span>
          </button>
        {/if}

        <div class="dialog-actions">
          {#if sceneModal.id}
            <button
              type="button"
              class="ghost danger"
              onclick={() => {
                const sc = scenes.find((s) => s.id === sceneModal?.id);
                sceneModal = null;
                if (sc) deleteSceneWithUndo(sc);
              }}
            >
              <Icon name="trash" size={14} /> Delete
            </button>
          {/if}
          <span class="spacer"></span>
          <button type="button" class="ghost" onclick={() => (sceneModal = null)}>Cancel</button>
          <button type="submit" class="primary" disabled={!sceneModal.name.trim()}>
            {sceneModal.id ? 'Save' : 'Save scene'}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

{#if addingLight}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (addingLight = false)}>
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-label="Add light by IP"
      use:modalFocusTrap
    >
      <form class="dialog-form" onsubmit={(e) => { e.preventDefault(); addLight(); }}>
        <h2>Add light by IP</h2>
        <p class="dialog-text">Use this if a light doesn't show up automatically. Find its IP address in your router's device list.</p>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="field" bind:value={ipDraft} placeholder="192.168.1.50" autofocus />
        <div class="dialog-actions">
          <span class="spacer"></span>
          <button type="button" class="ghost" onclick={() => (addingLight = false)}>Cancel</button>
          <button type="submit" class="primary">Add</button>
        </div>
      </form>
    </div>
  </div>
{/if}

{#if configOpen}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (configOpen = false)}>
    <div class="dialog config-dialog" role="dialog" aria-modal="true" aria-label="Configuration" use:modalFocusTrap>
      <div class="log-head">
        <h2>Configuration</h2>
        <button type="button" class="ghost icon-btn" title="Close" onclick={() => (configOpen = false)}>
          <Icon name="x" size={16} />
        </button>
      </div>

      <p class="dialog-text">
        Nicknames, groups, and scenes live in a plain JSON file. Symlink it or point Groovie at your dotfiles repo — external edits reload automatically.
      </p>

      <div class="prefs-card">
        <span class="path-label">Appearance &amp; startup</span>
        <div class="pref-row theme-row">
          <div class="pref-copy">
            <strong>Theme</strong>
            <span>Match your system appearance or lock dark or light mode</span>
          </div>
          <div class="segmented theme-seg" role="radiogroup" aria-label="Theme">
            <button
              type="button"
              role="radio"
              aria-checked={theme === 'system'}
              class:sel={theme === 'system'}
              onclick={() => selectTheme('system')}
            >
              System
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={theme === 'dark'}
              class:sel={theme === 'dark'}
              onclick={() => selectTheme('dark')}
            >
              Dark
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={theme === 'light'}
              class:sel={theme === 'light'}
              onclick={() => selectTheme('light')}
            >
              Light
            </button>
          </div>
        </div>
        <label class="pref-row">
          <div class="pref-copy">
            <strong>Launch at login</strong>
            <span>Start Groovie automatically when you sign in</span>
          </div>
          <span class="switch">
            <input
              type="checkbox"
              checked={desktopPrefs.launchAtLogin}
              onchange={(e) => updateDesktopPref('launchAtLogin', e.currentTarget.checked)}
            />
            <span></span>
          </span>
        </label>
        <label class="pref-row">
          <div class="pref-copy">
            <strong>Start minimized to tray</strong>
            <span>Keep the main window hidden in the system tray on launch</span>
          </div>
          <span class="switch">
            <input
              type="checkbox"
              checked={desktopPrefs.startMinimized}
              onchange={(e) => updateDesktopPref('startMinimized', e.currentTarget.checked)}
            />
            <span></span>
          </span>
        </label>
      </div>

      <div class="path-card">
        <div class="path-meta">
          <span class="path-label">Version &amp; updates</span>
          <div class="path-btns">
            {#if configInfo?.pinnedVersion && version && configInfo.pinnedVersion !== version && !editingPin}
              <button type="button" class="primary sm-btn" disabled={updating} onclick={installConfiguredPin}>
                <Icon name="download" size={12} />
                {updating ? updateLabel : `Install v${configInfo.pinnedVersion} & restart`}
              </button>
            {/if}
            {#if configInfo?.pinnedVersion || configInfo?.pinnedVersionError}
              <button type="button" class="ghost sm-btn" disabled={updating} onclick={unpinVersion}>
                Unpin
              </button>
            {/if}
            <button type="button" class="ghost sm-btn" disabled={updating} onclick={togglePinEditor}>
              <Icon name="pin" size={13} />
              {editingPin ? 'Close' : configInfo?.pinnedVersion ? 'Change…' : 'Pin version…'}
            </button>
          </div>
        </div>

        {#if configInfo?.pinnedVersionError}
          <div class="pin-warn">
            <Icon name="info" size={13} />
            <span>{configInfo.pinnedVersionError}</span>
          </div>
        {:else if configInfo?.pinnedVersion}
          <div class="pin-status">
            <strong>Pinned to <code>v{configInfo.pinnedVersion}</code></strong>
            <span>
              {configInfo.pinnedVersion === version
                ? 'Update suggestions are paused while pinned.'
                : `Running v${version} — install v${configInfo.pinnedVersion} to match groovie.json.`}
            </span>
          </div>
        {:else}
          <div class="pin-status">
            <strong>Suggest updates automatically</strong>
            <span>
              Running <code>v{version || '…'}</code>{#if update && !update.pinned}{' · '}<code>v{update.version}</code> available{/if}
            </span>
          </div>
        {/if}

        {#if editingPin}
          <form class="path-form" onsubmit={(e) => { e.preventDefault(); applyPinVersion(); }}>
            <select
              class="field"
              bind:value={pinChoice}
              disabled={updating || loadingReleases}
              aria-label="Target release version"
            >
              {#each versionChoices as rel (rel.version)}
                <option value={rel.version}>
                  v{rel.version}{rel.version === version ? ' · Installed' : ''}{rel.isLatest ? ' · Latest' : ''}{rel.publishedAt ? ` (${fmtReleaseDate(rel.publishedAt)})` : ''}
                </option>
              {/each}
              <option value="custom">Specify version manually…</option>
            </select>

            {#if pinChoice === 'custom'}
              <input
                class="field mono-field"
                bind:value={customPinDraft}
                placeholder={minPinnable}
                disabled={updating}
                aria-label="Pinned version (x.y.z)"
              />
            {/if}

            {#if loadingReleases}
              <span class="path-hint">Checking published GitHub Releases…</span>
            {:else if releasesError}
              <span class="path-hint">
                Could not fetch release list ({releasesError}); you can still pin the current version or enter any release ≥ v{minPinnable}.
              </span>
            {/if}

            <div class="path-form-foot">
              <span class="path-hint">
                {#if selectedPinTarget && version && selectedPinTarget !== version}
                  Writes <code>"pinned_version": "{selectedPinTarget}"</code> to <code>groovie.json</code>, downloads the signed installer, and restarts.
                {:else}
                  Locks Groovie to this release in <code>groovie.json</code> (earliest pinnable version is <code>v{minPinnable}</code>).
                {/if}
              </span>
              <span class="spacer"></span>
              <button
                type="submit"
                class="primary sm-btn"
                disabled={updating || !selectedPinTarget || (selectedPinTarget === version && configInfo?.pinnedVersion === selectedPinTarget)}
              >
                {#if updating}
                  {updateLabel}
                {:else if selectedPinTarget && version && selectedPinTarget !== version}
                  Pin &amp; restart into v{selectedPinTarget}
                {:else}
                  Pin v{selectedPinTarget || minPinnable}
                {/if}
              </button>
            </div>
          </form>
        {/if}
      </div>

      <div class="path-card">
        <div class="path-meta">
          <span class="path-label">{configInfo?.isCustom ? 'Custom path' : 'Config file'}</span>
          <div class="path-btns">
            {#if configInfo?.path}
              <button type="button" class="ghost sm-btn" onclick={copyConfigPath}>
                <Icon name={copiedPath ? 'check' : 'copy'} size={13} />
                {copiedPath ? 'Copied' : 'Copy path'}
              </button>
            {/if}
            {#if isTauri && configInfo?.path}
              <button type="button" class="ghost sm-btn" onclick={() => run((a) => a.openConfigFolder())}>
                <Icon name="folder" size={13} /> Open folder
              </button>
            {/if}
            <button
              type="button"
              class="ghost sm-btn"
              onclick={() => {
                editingPath = !editingPath;
                pathDraft = configInfo?.path ?? '';
              }}
            >
              <Icon name="pencil" size={13} /> {editingPath ? 'Close' : 'Change…'}
            </button>
          </div>
        </div>

        <code class="path-value">{configInfo?.path ?? 'In-memory'}</code>
        {#if configInfo?.symlinkTarget}
          <div class="symlink-note">→ <code>{configInfo.symlinkTarget}</code></div>
        {/if}

        {#if editingPath}
          <form class="path-form" onsubmit={(e) => { e.preventDefault(); saveCustomPath(); }}>
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="field mono-field"
              bind:value={pathDraft}
              placeholder="~/dotfiles/groovie/groovie.json"
              autofocus
            />
            <div class="path-form-foot">
              <span class="path-hint">Existing files are loaded; new paths are seeded with your current config.</span>
              <span class="spacer"></span>
              {#if configInfo?.isCustom}
                <button type="button" class="ghost sm-btn" onclick={resetConfigPath}>Use default</button>
              {/if}
              <button type="submit" class="primary sm-btn" disabled={!pathDraft.trim()}>Save path</button>
            </div>
          </form>
        {/if}
      </div>

      <pre class="config-box">{configInfo?.json ?? ''}</pre>

      <div class="dialog-actions log-foot">
        <button type="button" class="ghost" onclick={reloadConfigFromDisk}>
          <Icon name="refresh" size={14} /> Reload from disk
        </button>
        <span class="spacer"></span>
        <button type="button" class="primary" onclick={copyConfigJson}>
          <Icon name={copiedConfig ? 'check' : 'copy'} size={14} />
          {copiedConfig ? 'Copied' : 'Copy JSON'}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if logsOpen}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (logsOpen = false)}>
    <div class="dialog log-dialog" role="dialog" aria-modal="true" aria-label="Activity & network log" use:modalFocusTrap>
      <div class="log-head">
        <h2>Activity &amp; network log</h2>
        <button type="button" class="ghost icon-btn" title="Close" onclick={() => (logsOpen = false)}>
          <Icon name="x" size={16} />
        </button>
      </div>

      {#if devices.length}
        <div class="dev-table">
          {#each devices as d (d.id)}
            <div class="dev-row">
              <strong>{d.name}</strong>
              <code>{d.sku}</code>
              <code>{d.ip}</code>
              <code class="dim">{d.id}</code>
              {#if d.firmware}<span class="dim">{d.firmware}</span>{/if}
              <span class="dev-state">{fmtDeviceSummary(d)}</span>
            </div>
          {/each}
        </div>
      {/if}

      <div class="log-box" bind:this={logBox}>
        {#each shownLogs as l, idx (idx)}
          <div class="log-line" class:hl={l.dir === 'action' || l.dir === 'state'}>
            <span class="ts">{fmtTime(l.tsMs)}</span>
            <span class="tag tag-{l.dir}">{dirTag(l.dir)}</span>
            <span class="msg">
              {#if l.device || l.addr}
                <span class="who">{[l.device, l.addr].filter(Boolean).join(' · ')}</span>
              {/if}
              {l.message}
            </span>
          </div>
        {:else}
          <p class="log-empty">No activity recorded yet. Toggle a light to capture packets.</p>
        {/each}
      </div>

      <div class="dialog-actions log-foot">
        <label class="poll-toggle">
          <input type="checkbox" bind:checked={hidePolls} />
          <span>Hide background polls</span>
        </label>
        <span class="spacer"></span>
        <button type="button" class="ghost" onclick={clearLogEntries} disabled={!logs.length}>Clear</button>
        <button type="button" class="primary" onclick={copyLogs}>
          <Icon name={copied ? 'check' : 'copy'} size={14} />
          {copied ? 'Copied' : 'Copy report'}
        </button>
      </div>
    </div>
  </div>
{/if}

<svelte:window
  onkeydown={(e) => {
    if (e.key !== 'Escape') return;
    if (logsOpen) logsOpen = false;
    else if (configOpen) configOpen = false;
    else if (sceneModal) sceneModal = null;
    else if (editing) editing = null;
    else if (addingLight) addingLight = false;
  }}
/>

{#if toast}
  <div class="toast" role="status">
    <span>{toast}</span>
    {#if toastAction}
      <button type="button" class="toast-btn" onclick={() => toastAction?.run()}>
        {toastAction.label}
      </button>
    {/if}
  </div>
{/if}

<style>
  .app { display: grid; grid-template-columns: 232px 1fr; height: 100vh; }

  aside {
    background: var(--sidebar);
    border-right: 1px solid var(--border);
    padding: 16px 10px 12px;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
    padding: 2px 10px 16px;
  }
  .brand svg { width: 20px; height: 20px; }

  nav { display: flex; flex-direction: column; gap: 1px; flex: 1; overflow-y: auto; }
  .nav-item {
    all: unset;
    box-sizing: border-box;
    width: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 10px;
    border-radius: 7px;
    color: var(--text-2);
    cursor: pointer;
  }
  .nav-item span { flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .nav-item small { font-size: 12px; color: var(--text-3); font-variant-numeric: tabular-nums; }
  .nav-item:hover { background: var(--hover); color: var(--text); }
  .nav-item.active { background: var(--active); color: var(--text); }
  .nav-item:focus-visible { outline: 2px solid var(--focus); }

  .sync-nav {
    display: flex;
    align-items: center;
    border-radius: 7px;
    padding-right: 7px;
  }
  .sync-nav:hover { background: var(--hover); }
  .sync-nav:hover .nav-item { background: transparent; color: var(--text); }
  .sync-nav.active { background: var(--active); }
  .sync-nav.active .nav-item { background: transparent; color: var(--text); }
  .sync-nav.live :global(svg) { color: var(--accent); }
  .nav-switch {
    width: 30px;
    height: 18px;
  }
  .nav-switch span { border-radius: 9px; }
  .nav-switch span::before {
    width: 14px;
    height: 14px;
    left: 2px;
    top: 2px;
  }
  .nav-switch input:checked + span::before { transform: translateX(12px); }

  .section {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 4px 4px 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-3);
  }
  .empty-hint { margin: 2px 10px 4px; font-size: 12.5px; color: var(--text-3); line-height: 1.4; }
  .scene { display: flex; align-items: center; border-radius: 7px; }
  .scene .nav-item { flex: 1; width: auto; min-width: 0; }
  .scene-actions { display: flex; align-items: center; gap: 1px; opacity: 0; }
  .scene:hover .scene-actions, .scene-actions:focus-within { opacity: 1; }
  .scene:hover .scene-count, .scene:focus-within .scene-count { display: none; }

  footer { display: flex; flex-direction: column; gap: 8px; padding: 10px 4px 0 3px; }
  .foot-row { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
  .foot-left { display: flex; align-items: center; gap: 2px; min-width: 0; }
  .version-link {
    height: 26px;
    padding: 0 7px;
    gap: 3px;
    font-size: 12px;
    border-radius: 6px;
    color: var(--text-3);
  }
  .version-link :global(svg) { opacity: 0.55; transition: opacity 0.12s, transform 0.12s; }
  .version-link:hover :global(svg) { opacity: 1; transform: translate(1px, -1px); }
  .pin-pill {
    height: 22px;
    padding: 0 6px;
    gap: 3px;
    font-size: 11px;
    border-radius: 5px;
    color: var(--accent);
    background: #ffb02014;
  }
  .pin-pill:hover { background: #ffb02024; color: var(--accent); }
  .foot-tools { display: flex; align-items: center; gap: 2px; }
  .foot-icon {
    width: 26px;
    height: 26px;
    padding: 0;
    justify-content: center;
    border-radius: 6px;
    color: var(--text-3);
  }
  .update {
    position: relative;
    isolation: isolate;
    overflow: hidden;
    justify-content: center;
    background: var(--accent);
    border-color: var(--accent);
    color: #1a1204;
    font-weight: 550;
    font-variant-numeric: tabular-nums;
  }
  .update:hover:not(:disabled) {
    background: var(--accent);
    border-color: var(--accent);
    filter: brightness(1.05);
  }
  .update.busy:disabled {
    opacity: 1;
    background: var(--surface-2);
    border-color: #ffb02066;
    color: var(--text);
  }
  .update.busy::before {
    content: '';
    position: absolute;
    inset: 0 auto 0 0;
    z-index: -1;
    width: var(--pct, 0%);
    background: #ffb02038;
    transition: width 0.15s ease-out;
  }

  main { overflow-y: auto; padding: 22px 28px 40px; }
  main > header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 20px;
  }
  h1 { margin: 0; font-size: 20px; font-weight: 600; letter-spacing: -0.015em; }
  .sub { margin: 2px 0 0; color: var(--text-2); font-size: 13px; }
  .actions { display: flex; align-items: center; gap: 6px; }
  .spin :global(svg) { animation: spin 0.9s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 14px; align-items: start; }
  .banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 14px 16px;
    margin-bottom: 16px;
  }
  .banner p { margin: 3px 0 0; color: var(--text-2); font-size: 13px; }
  .banner-actions { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }
  .empty {
    grid-column: 1 / -1;
    max-width: 440px;
    margin: 80px auto;
    text-align: center;
    color: var(--text-2);
    line-height: 1.5;
  }
  .empty-title { color: var(--text); font-weight: 550; font-size: 15px; }

  .backdrop {
    position: fixed;
    inset: 0;
    background: #0009;
    display: grid;
    place-items: center;
    animation: fade 0.12s ease-out;
  }
  .dialog {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 18px;
    width: min(380px, 92vw);
    display: flex;
    flex-direction: column;
    gap: 12px;
    box-shadow: 0 20px 60px #0007;
  }
  .dialog-form { display: flex; flex-direction: column; gap: 12px; }
  .scene-dialog { width: min(420px, 92vw); }
  .dialog h2 { margin: 0; font-size: 16px; font-weight: 600; }
  .dialog-text { margin: -4px 0 0; color: var(--text-2); font-size: 13px; line-height: 1.45; }
  .scope-block {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .scope-hint {
    margin: 2px 0 0;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .capture-btn {
    width: 100%;
    justify-content: center;
    background: #ffb02016;
    border-color: #ffb0204d;
    color: var(--accent);
    font-weight: 550;
  }
  .capture-btn:hover:not(:disabled) {
    background: #ffb02026;
  }
  .checks { display: flex; flex-direction: column; gap: 2px; max-height: 220px; overflow-y: auto; }
  .check { display: flex; align-items: center; gap: 10px; padding: 7px 8px; border-radius: 7px; cursor: pointer; }
  .check:hover { background: var(--hover); }
  .check span { flex: 1; }
  .check small { color: var(--text-3); font-size: 12px; }
  .dialog-actions { display: flex; align-items: center; gap: 6px; margin-top: 4px; }
  .spacer { flex: 1; }

  .config-dialog { width: min(640px, 94vw); max-height: 88vh; overflow-y: auto; }
  .prefs-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
  }
  .pref-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    cursor: pointer;
    user-select: none;
  }
  .pref-copy { display: flex; flex-direction: column; gap: 1px; }
  .pref-copy strong { font-size: 13px; font-weight: 550; color: var(--text); }
  .pref-copy span { font-size: 12px; color: var(--text-2); }
  .theme-row { cursor: default; }
  .theme-seg { min-width: 192px; flex-shrink: 0; }
  .path-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
  }
  .path-meta { display: flex; align-items: center; justify-content: space-between; gap: 8px; flex-wrap: wrap; }
  .path-label { font-size: 11.5px; font-weight: 550; color: var(--text-3); text-transform: uppercase; letter-spacing: 0.04em; }
  .path-btns { display: flex; align-items: center; gap: 2px; margin-right: -4px; }
  .sm-btn { height: 25px; padding: 0 8px; font-size: 12px; border-radius: 6px; }
  .path-value {
    font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace;
    font-size: 12px;
    color: var(--text);
    word-break: break-all;
  }
  .symlink-note { font-size: 11.5px; color: var(--text-3); word-break: break-all; }
  .symlink-note code { font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace; color: var(--text-2); }
  .path-form {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding-top: 8px;
    margin-top: 2px;
    border-top: 1px solid var(--border);
  }
  .mono-field {
    font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace;
    font-size: 12px;
    background: var(--surface);
  }
  .path-form-foot { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
  .path-hint { font-size: 11.5px; color: var(--text-3); line-height: 1.4; }
  .path-hint code { font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace; color: var(--text-2); }
  .pin-status { display: flex; flex-direction: column; gap: 2px; }
  .pin-status strong { font-size: 13px; font-weight: 550; color: var(--text); }
  .pin-status span { font-size: 12px; color: var(--text-2); }
  .pin-status code { font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace; font-size: 11.5px; }
  .pin-warn {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--accent);
    line-height: 1.4;
  }
  .config-box {
    margin: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 10px 12px;
    max-height: min(240px, 34vh);
    overflow: auto;
    font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-2);
  }

  .log-dialog { width: min(760px, 94vw); max-height: 86vh; }
  .log-head { display: flex; align-items: center; justify-content: space-between; margin: -2px -4px 0 0; }
  .dev-table {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 9px 11px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
    font-size: 12px;
  }
  .dev-row { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  .dev-row strong { font-weight: 550; }
  .dev-row code { font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace; font-size: 11.5px; color: var(--text-2); }
  .dev-row .dim { color: var(--text-3); }
  .dev-state { margin-left: auto; color: var(--text-2); font-variant-numeric: tabular-nums; }

  .log-box {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 8px 10px;
    height: min(360px, 48vh);
    overflow-y: auto;
    font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace;
    font-size: 11.5px;
    line-height: 1.5;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .log-line { display: grid; grid-template-columns: 82px 34px 1fr; gap: 8px; align-items: baseline; color: var(--text-2); }
  .log-line.hl { color: var(--text); }
  .ts { color: var(--text-3); font-variant-numeric: tabular-nums; }
  .tag {
    font-size: 10.5px;
    font-weight: 600;
    text-align: center;
    padding: 0 4px;
    border-radius: 4px;
    background: var(--track);
    color: var(--text-2);
  }
  .tag-action, .tag-state { background: #ffb02026; color: var(--accent); }
  .msg { word-break: break-word; }
  .who { color: var(--text); margin-right: 6px; }
  .log-empty { margin: auto; color: var(--text-3); font-family: inherit; font-size: 13px; }
  .log-foot { flex-wrap: wrap; }
  .poll-toggle { display: inline-flex; align-items: center; gap: 7px; font-size: 12.5px; color: var(--text-2); cursor: pointer; user-select: none; }

  .toast {
    position: fixed;
    bottom: 18px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    padding: 9px 14px;
    border-radius: 10px;
    font-size: 13px;
    box-shadow: 0 10px 30px #0006;
    max-width: min(520px, 90vw);
    animation: fade 0.15s ease-out;
  }
  .toast-btn {
    height: 24px;
    padding: 0 9px;
    font-size: 12px;
    font-weight: 600;
    border-radius: 6px;
    background: #ffb02024;
    border-color: #ffb02055;
    color: var(--accent);
  }
  .toast-btn:hover:not(:disabled) {
    background: #ffb02036;
  }
  @keyframes fade { from { opacity: 0; } }

  @media (max-width: 680px) {
    .app { grid-template-columns: 1fr; height: auto; }
    aside { border-right: 0; border-bottom: 1px solid var(--border); }
    main { padding: 18px 16px 32px; }
  }
</style>
