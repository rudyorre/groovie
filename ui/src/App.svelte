<script lang="ts">
  import { onMount, tick } from 'svelte';
  import {
    api,
    isIos,
    isTauri,
    toHex,
    type ConfigInfo,
    type Device,
    type Group,
    type LogEntry,
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
  import ScreenSyncView from './lib/ScreenSyncView.svelte';

  let devices = $state<Device[]>([]);
  let groups = $state<Group[]>([]);
  let scenes = $state<Scene[]>([]);
  let syncState = $state<SyncState | null>(null);
  let syncPreview = $state<SyncPreview | null>(null);
  /** 'all', 'sync', or a group id */
  let view = $state('all');
  let toast = $state('');
  let startupError = $state('');
  let version = $state('');
  let update = $state<Update | null>(null);
  let updateProgress = $state<UpdateProgress | null>(null);

  // Dialogs / inline editors / mobile sheet
  let drawerOpen = $state(false);
  let editing = $state<Group | null>(null); // id '' = new group
  let addingLight = $state(false);
  let ipDraft = $state('');
  let namingScene = $state(false);
  let savingSceneModal = $state(false);
  let sceneDraft = $state('');
  let scanning = $state(false);
  let logsOpen = $state(false);
  let logs = $state<LogEntry[]>([]);
  let hidePolls = $state(true);
  let copied = $state(false);
  let logBox = $state<HTMLElement | null>(null);
  let configOpen = $state(false);
  let configInfo = $state<ConfigInfo | null>(null);
  let editingPath = $state(false);
  let pathDraft = $state('');
  let copiedPath = $state(false);
  let copiedConfig = $state(false);
  let editingJson = $state(false);
  let jsonDraft = $state('');
  let jsonError = $state('');
  const canShareConfig = typeof navigator !== 'undefined' && typeof navigator.share === 'function';

  const group = $derived(groups.find((g) => g.id === view));
  const shown = $derived(group ? devices.filter((d) => group.members.includes(d.id)) : devices);
  const target = $derived<Target>(group ? { type: 'group', id: group.id } : { type: 'all' });
  const onCount = $derived(shown.filter((d) => d.online && d.state?.on).length);
  const shownLogs = $derived(hidePolls ? logs.filter((l) => !l.poll) : logs);
  const updating = $derived(updateProgress !== null);
  const updatePct = $derived(
    updateProgress?.stage === 'downloading' ? (updateProgress.percent ?? 15) : updateProgress ? 100 : 0,
  );
  const updateLabel = $derived(
    !updateProgress
      ? `Update to ${update?.version ?? ''}`
      : updateProgress.stage === 'downloading'
        ? updateProgress.percent !== null
          ? `Downloading ${updateProgress.percent}%`
          : 'Downloading…'
        : updateProgress.stage === 'installing'
          ? 'Installing…'
          : 'Restarting…',
  );

  let toastTimer: ReturnType<typeof setTimeout>;
  function showToast(msg: string) {
    toast = msg;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ''), 4000);
  }

  async function run<T>(f: (a: Awaited<typeof api>) => Promise<T>) {
    try {
      return await f(await api);
    } catch (e) {
      showToast(String(e));
    }
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

  async function toggleSync(enabled: boolean) {
    const next = await run((a) => a.setSyncEnabled(enabled));
    if (next) {
      syncState = next;
      if (!next.active) syncPreview = null;
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
    let lastUpdateCheck = 0;
    const maybeCheckUpdate = async () => {
      if (update || updating || Date.now() - lastUpdateCheck < 60_000) return;
      lastUpdateCheck = Date.now();
      const a = await api;
      update = await a.checkUpdate().catch(() => null);
    };
    const onFocus = () => {
      maybeCheckUpdate();
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
          if (configOpen) {
            a.getConfigInfo().then((info) => (configInfo = info)).catch(() => {});
          }
        } else if (e.type === 'logged' && logsOpen) {
          logs = [...logs.slice(-499), e.entry];
        }
      });
      await a.setWindowActive(!document.hidden, false).catch(() => {});
      version = await a.appVersion().catch(() => '');
      await maybeCheckUpdate();
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
    drawerOpen = false;
    configOpen = true;
    editingPath = false;
    editingJson = false;
    jsonError = '';
    copiedPath = false;
    copiedConfig = false;
    const info = await run((a) => a.getConfigInfo());
    if (info) {
      configInfo = info;
      pathDraft = info.path ?? '';
      jsonDraft = info.json;
    }
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

  async function shareConfigJson() {
    if (!configInfo || !canShareConfig) return;
    try {
      const file =
        typeof File !== 'undefined'
          ? new File([configInfo.json + '\n'], 'groovie.json', { type: 'application/json' })
          : null;
      if (file && typeof navigator.canShare === 'function' && navigator.canShare({ files: [file] })) {
        await navigator.share({ files: [file], title: 'groovie.json' });
      } else {
        await navigator.share({ title: 'groovie.json', text: configInfo.json });
      }
    } catch (e) {
      if ((e as { name?: string })?.name !== 'AbortError') {
        showToast(`Could not share: ${e}`);
      }
    }
  }

  function startEditJson() {
    jsonDraft = configInfo?.json ?? '{\n  "version": 1\n}';
    jsonError = '';
    editingJson = true;
  }

  async function pasteClipboardJson() {
    try {
      const text = await navigator.clipboard.readText();
      if (text.trim()) {
        jsonDraft = text;
        jsonError = '';
      } else {
        showToast('Clipboard is empty');
      }
    } catch {
      showToast('Tap inside the editor and use Paste');
    }
  }

  async function saveEditedJson() {
    jsonError = '';
    try {
      const parsed = JSON.parse(jsonDraft);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        jsonError = 'Root value must be a JSON object.';
        return;
      }
    } catch (e) {
      jsonError = e instanceof Error ? e.message : String(e);
      return;
    }
    try {
      const a = await api;
      const info = await a.saveConfigJson(jsonDraft);
      configInfo = info;
      jsonDraft = info.json;
      pathDraft = info.path ?? '';
      editingJson = false;
      await reload();
      showToast('Saved configuration');
    } catch (e) {
      jsonError = String(e);
    }
  }

  async function reloadConfigFromDisk() {
    const info = await run((a) => a.reloadConfig());
    if (info) {
      configInfo = info;
      pathDraft = info.path ?? '';
      jsonDraft = info.json;
      editingJson = false;
      jsonError = '';
      await reload();
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
      jsonDraft = info.json;
      editingPath = false;
      await reload();
      showToast('Updated config file location');
    }
  }

  async function resetConfigPath() {
    const info = await run((a) => a.setConfigPath(null));
    if (info) {
      configInfo = info;
      pathDraft = info.path ?? '';
      jsonDraft = info.json;
      editingPath = false;
      await reload();
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

  async function saveScene() {
    const name = sceneDraft.trim();
    namingScene = false;
    savingSceneModal = false;
    sceneDraft = '';
    if (name) {
      const created = await run((a) => a.saveScene(name));
      if (created) showToast(`Saved scene "${created.name}"`);
    }
  }

  async function addLight() {
    const ip = ipDraft.trim();
    addingLight = false;
    ipDraft = '';
    if (ip) await run((a) => a.addIp(ip));
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
  {#if drawerOpen}
    <div class="drawer-backdrop" role="presentation" onclick={() => (drawerOpen = false)}></div>
  {/if}

  <aside class:open={drawerOpen}>
    <div class="drawer-grab" aria-hidden="true"></div>
    <div class="brand-row">
      <div class="brand">
        <svg viewBox="0 0 1024 1024" aria-hidden="true">
          <path d="M768 466.9A260 260 0 1 1 557.1 255.9" fill="none" stroke="currentColor" stroke-width="120" stroke-linecap="round" />
          <circle cx="695.8" cy="328.2" r="60" fill="#FFB020" />
        </svg>
        Groovie
      </div>
      <button type="button" class="ghost icon-btn drawer-close" aria-label="Close menu" onclick={() => (drawerOpen = false)}>
        <Icon name="x" size={16} />
      </button>
    </div>

    <nav>
      <button class="nav-item" class:active={view === 'all'} onclick={() => { view = 'all'; drawerOpen = false; }}>
        <Icon name="grid" /> <span>All lights</span> <small>{devices.length}</small>
      </button>

      {#if syncState?.supported}
        <div class="sync-nav" class:active={view === 'sync'} class:live={syncState.active}>
          <button class="nav-item" onclick={() => { view = 'sync'; drawerOpen = false; }}>
            <Icon name="monitor" /> <span>Screen sync</span>
          </button>
          <label class="switch nav-switch" title={syncState.active ? 'Stop screen sync' : 'Start screen sync'}>
            <input
              type="checkbox"
              checked={syncState.active}
              onchange={(e) => toggleSync(e.currentTarget.checked)}
            />
            <span></span>
          </label>
        </div>
      {/if}

      <div class="section">
        <span>Groups</span>
        <button class="ghost icon-btn" title="New group" onclick={() => { drawerOpen = false; editing = { id: '', name: '', members: [] }; }}>
          <Icon name="plus" size={14} />
        </button>
      </div>
      {#each groups as g (g.id)}
        <button class="nav-item" class:active={view === g.id} onclick={() => { view = g.id; drawerOpen = false; }}>
          <Icon name="folder" /> <span>{g.name}</span> <small>{g.members.length}</small>
        </button>
      {:else}
        <p class="empty-hint">Group lights to control them together.</p>
      {/each}

      <div class="section">
        <span>Scenes</span>
        <button class="ghost icon-btn" title="Save current look as a scene" onclick={() => (namingScene = true)}>
          <Icon name="plus" size={14} />
        </button>
      </div>
      {#if namingScene}
        <form onsubmit={(e) => { e.preventDefault(); saveScene(); }}>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="field"
            bind:value={sceneDraft}
            placeholder="Scene name, then Enter"
            autofocus
            onblur={() => (namingScene = false)}
            onkeydown={(e) => e.key === 'Escape' && (namingScene = false)}
          />
        </form>
      {/if}
      {#each scenes as sc (sc.id)}
        <div class="scene">
          <button class="nav-item" title="Apply scene" onclick={() => { drawerOpen = false; run((a) => a.applyScene(sc.id)); }}>
            <Icon name="sparkle" /> <span>{sc.name}</span>
          </button>
          <button class="ghost icon-btn del" title="Delete scene" onclick={() => run((a) => a.deleteScene(sc.id))}>
            <Icon name="x" size={14} />
          </button>
        </div>
      {:else}
        {#if !namingScene}<p class="empty-hint">Save the current look with +.</p>{/if}
      {/each}

      <div class="drawer-only">
        <div class="section"><span>Tools</span></div>
        <button class="nav-item" onclick={() => { drawerOpen = false; addingLight = true; }}>
          <Icon name="plus" /> <span>Add light by IP</span>
        </button>
        <button class="nav-item" onclick={openConfig}>
          <Icon name="sliders" /> <span>Configuration (groovie.json)</span>
        </button>
        <button class="nav-item" onclick={() => { drawerOpen = false; openLogs(); }}>
          <Icon name="terminal" /> <span>Activity &amp; network log</span>
        </button>
      </div>
    </nav>

    <footer>
      {#if update}
        <button
          class="update"
          class:busy={updating}
          class:spin={updateProgress !== null && updateProgress.stage !== 'downloading'}
          style:--pct="{updatePct}%"
          onclick={installUpdate}
          disabled={updating}
        >
          <Icon name={updateProgress && updateProgress.stage !== 'downloading' ? 'refresh' : 'download'} size={14} />
          <span>{updateLabel}</span>
        </button>
      {/if}
      <div class="foot-row">
        <button
          class="ghost version-link"
          title="View Groovie on GitHub"
          onclick={() => run((a) => a.openGithub())}
        >
          <span>{isTauri ? `v${version}` : 'Demo'}</span>
          <Icon name="external" size={11} />
        </button>
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
            <strong>Could not start local network listener</strong>
            <p>{startupError}</p>
          </div>
          <button onclick={rescan}><Icon name="refresh" size={14} /> Retry</button>
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
      <div class="mobile-top">
        <div class="mobile-bar">
          <div class="mobile-title">
            <svg class="mobile-logo" viewBox="0 0 1024 1024" aria-hidden="true">
              <path d="M768 466.9A260 260 0 1 1 557.1 255.9" fill="none" stroke="currentColor" stroke-width="120" stroke-linecap="round" />
              <circle cx="695.8" cy="328.2" r="60" fill="#FFB020" />
            </svg>
            <div class="mobile-heading">
              <h1>{group?.name ?? 'All lights'}</h1>
              <p class="sub">
                {shown.length} {shown.length === 1 ? 'light' : 'lights'}{#if shown.length}{' · '}{onCount} on{/if}
              </p>
            </div>
          </div>
          <div class="mobile-actions">
            <button class="ghost icon-btn" class:spin={scanning} title="Scan for lights" aria-label="Scan for lights" onclick={rescan}>
              <Icon name="refresh" size={16} />
            </button>
            <button
              class="mobile-power"
              onclick={() => run((a) => a.apply(target, { type: 'power', value: onCount === 0 }))}
              disabled={!shown.length}
            >
              <Icon name="power" size={14} /> {onCount ? 'All off' : 'All on'}
            </button>
            <button class="ghost icon-btn" title="Menu & tools" aria-label="Menu & tools" onclick={() => (drawerOpen = true)}>
              <Icon name="menu" size={17} />
            </button>
          </div>
        </div>

        <div class="mobile-strips">
          <div class="pill-strip" role="tablist" aria-label="Light groups">
            <button role="tab" aria-selected={view === 'all'} class="pill" class:sel={view === 'all'} onclick={() => (view = 'all')}>
              All <small>{devices.length}</small>
            </button>
            {#each groups as g (g.id)}
              <button role="tab" aria-selected={view === g.id} class="pill" class:sel={view === g.id} onclick={() => (view = g.id)}>
                {g.name} <small>{g.members.length}</small>
              </button>
            {/each}
            {#if group}
              <button class="pill ghost-pill" onclick={() => (editing = structuredClone($state.snapshot(group)))}>
                <Icon name="pencil" size={12} /> Edit
              </button>
            {/if}
            <button class="pill ghost-pill" onclick={() => (editing = { id: '', name: '', members: [] })}>
              <Icon name="plus" size={12} /> Group
            </button>
          </div>

          {#if scenes.length || devices.some((d) => d.state)}
            <div class="pill-strip scene-strip" aria-label="Scenes">
              {#each scenes as sc (sc.id)}
                <button class="pill scene-pill" onclick={() => run((a) => a.applyScene(sc.id))}>
                  <Icon name="sparkle" size={12} /> {sc.name}
                </button>
              {/each}
              {#if devices.some((d) => d.state)}
                <button class="pill ghost-pill" onclick={() => { sceneDraft = ''; savingSceneModal = true; }}>
                  <Icon name="plus" size={12} /> Save scene
                </button>
              {/if}
            </div>
          {/if}
        </div>
      </div>

      <header class="desktop-header">
        <div>
          <h1>{group?.name ?? 'All lights'}</h1>
          <p class="sub">
            {shown.length} {shown.length === 1 ? 'light' : 'lights'}{#if shown.length}{' · '}{onCount} on{/if}
          </p>
        </div>
        <div class="actions">
          {#if group}
            <button class="ghost" onclick={() => (editing = structuredClone($state.snapshot(group)))}>
              <Icon name="pencil" size={14} /> Edit group
            </button>
          {/if}
          <button class="ghost icon-btn" title="Add light by IP" onclick={() => (addingLight = true)}><Icon name="plus" /></button>
          <button class="ghost icon-btn" class:spin={scanning} title="Search for lights" onclick={rescan}><Icon name="refresh" /></button>
          <button onclick={() => run((a) => a.apply(target, { type: 'power', value: onCount === 0 }))} disabled={!shown.length}>
            <Icon name="power" size={14} /> {onCount ? 'All off' : 'All on'}
          </button>
        </div>
      </header>

      {#if startupError}
        <div class="banner" role="alert">
          <div>
            <strong>Could not start local network listener</strong>
            <p>{startupError}</p>
          </div>
          <button onclick={rescan}><Icon name="refresh" size={14} /> Retry</button>
        </div>
      {/if}

      <section class="grid">
        {#each shown as d (d.id)}
          <DeviceCard
            device={d}
            syncing={Boolean(syncState?.active && syncState.activeDevices.includes(d.id))}
            onOpenSync={() => (view = 'sync')}
          />
        {:else}
          <div class="empty">
            {#if group}
              <p class="empty-title">This group is empty</p>
              <p>Use <b>Edit group</b> to choose which lights belong here.</p>
            {:else}
              <p class="empty-title">Looking for lights…</p>
              <p>Turn on <b>LAN Control</b> for each light in the Govee app (device settings), and make sure this
                device is on the same Wi‑Fi.{#if isIos} Allow <b>Local Network</b> access if iOS prompts.{/if} You can
                also add a light by its IP address{#if isIos} or paste your desktop <code>groovie.json</code> in Configuration{/if}.</p>
              <div class="empty-actions">
                <button onclick={() => (addingLight = true)}><Icon name="plus" size={14} /> Add by IP</button>
                <button class="ghost" onclick={openConfig}><Icon name="sliders" size={14} /> Configuration</button>
              </div>
            {/if}
          </div>
        {/each}
      </section>
    {/if}
  </main>
</div>

{#if editing}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (editing = null)}>
    <form class="dialog" onsubmit={(e) => { e.preventDefault(); saveGroup(); }}>
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
{/if}

{#if savingSceneModal}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (savingSceneModal = false)}>
    <form class="dialog" onsubmit={(e) => { e.preventDefault(); saveScene(); }}>
      <h2>Save scene</h2>
      <p class="dialog-text">Capture the current power, brightness, and colour of your lights as a one-tap scene.</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="field" bind:value={sceneDraft} placeholder="Scene name, e.g. Movie night" autofocus />
      <div class="dialog-actions">
        <span class="spacer"></span>
        <button type="button" class="ghost" onclick={() => (savingSceneModal = false)}>Cancel</button>
        <button type="submit" class="primary" disabled={!sceneDraft.trim()}>Save</button>
      </div>
    </form>
  </div>
{/if}

{#if addingLight}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (addingLight = false)}>
    <form class="dialog" onsubmit={(e) => { e.preventDefault(); addLight(); }}>
      <h2>Add light by IP</h2>
      <p class="dialog-text">Use this if a light doesn't show up automatically. Find its IP address in your router's device list.</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="field" bind:value={ipDraft} inputmode="decimal" placeholder="192.168.1.50" autofocus />
      <div class="dialog-actions">
        <span class="spacer"></span>
        <button type="button" class="ghost" onclick={() => (addingLight = false)}>Cancel</button>
        <button type="submit" class="primary">Add</button>
      </div>
    </form>
  </div>
{/if}

{#if configOpen}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (configOpen = false)}>
    <div class="dialog config-dialog" role="dialog" aria-modal="true" aria-label="Configuration">
      <div class="log-head">
        <h2>Configuration</h2>
        <button type="button" class="ghost icon-btn" title="Close" onclick={() => (configOpen = false)}>
          <Icon name="x" size={16} />
        </button>
      </div>

      <p class="dialog-text">
        {#if isIos}
          Nicknames, groups, and scenes live in <code>groovie.json</code> — accessible in the iOS <b>Files</b> app under
          <b>On My iPhone → Groovie</b>, or paste/share JSON directly below to sync with your desktop dotfiles.
        {:else}
          Nicknames, groups, and scenes live in a plain JSON file. Symlink it or point Groovie at your dotfiles repo —
          external edits reload automatically, or edit/paste JSON directly below.
        {/if}
      </p>

      <div class="path-card">
        <div class="path-meta">
          <span class="path-label">{isIos ? 'Files app location' : configInfo?.isCustom ? 'Custom path' : 'Config file'}</span>
          <div class="path-btns">
            {#if configInfo?.path}
              <button type="button" class="ghost sm-btn" onclick={copyConfigPath}>
                <Icon name={copiedPath ? 'check' : 'copy'} size={13} />
                {copiedPath ? 'Copied' : 'Copy path'}
              </button>
            {/if}
            {#if isTauri && !isIos && configInfo?.path}
              <button type="button" class="ghost sm-btn" onclick={() => run((a) => a.openConfigFolder())}>
                <Icon name="folder" size={13} /> Open folder
              </button>
            {/if}
            {#if !isIos}
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
            {/if}
          </div>
        </div>

        <code class="path-value">{isIos ? 'On My iPhone › Groovie › groovie.json' : (configInfo?.path ?? 'In-memory')}</code>
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

      {#if editingJson}
        <div class="json-editor-wrap">
          <div class="json-editor-bar">
            <span class="path-label">Edit / Paste groovie.json</span>
            <button type="button" class="ghost sm-btn" onclick={pasteClipboardJson}>
              <Icon name="paste" size={13} /> Paste clipboard
            </button>
          </div>
          <textarea
            class="config-editor"
            bind:value={jsonDraft}
            spellcheck="false"
            autocapitalize="off"
            autocomplete="off"
            aria-label="groovie.json editor"
          ></textarea>
          {#if jsonError}
            <p class="json-error" role="alert">{jsonError}</p>
          {/if}
        </div>

        <div class="dialog-actions log-foot">
          <span class="path-hint">Online lights stay active; newly added device IPs are probed right away.</span>
          <span class="spacer"></span>
          <button type="button" class="ghost" onclick={() => { editingJson = false; jsonError = ''; }}>Cancel</button>
          <button type="button" class="primary" onclick={saveEditedJson}>
            <Icon name="check" size={14} /> Save JSON
          </button>
        </div>
      {:else}
        <pre class="config-box">{configInfo?.json ?? ''}</pre>

        <div class="dialog-actions log-foot">
          <button type="button" class="ghost" onclick={reloadConfigFromDisk}>
            <Icon name="refresh" size={14} /> Reload
          </button>
          <button type="button" class="ghost" onclick={startEditJson}>
            <Icon name="pencil" size={14} /> Edit / Paste
          </button>
          <span class="spacer"></span>
          {#if canShareConfig}
            <button type="button" class="ghost" onclick={shareConfigJson}>
              <Icon name="share" size={14} /> Share
            </button>
          {/if}
          <button type="button" class="primary" onclick={copyConfigJson}>
            <Icon name={copiedConfig ? 'check' : 'copy'} size={14} />
            {copiedConfig ? 'Copied' : 'Copy JSON'}
          </button>
        </div>
      {/if}
    </div>
  </div>
{/if}

{#if logsOpen}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (logsOpen = false)}>
    <div class="dialog log-dialog" role="dialog" aria-modal="true" aria-label="Activity & network log">
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
    else if (editing) editing = null;
    else if (savingSceneModal) savingSceneModal = false;
    else if (addingLight) addingLight = false;
    else if (drawerOpen) drawerOpen = false;
  }}
/>

{#if toast}
  <div class="toast" role="status">{toast}</div>
{/if}

<style>
  .app { display: grid; grid-template-columns: 232px 1fr; height: 100vh; }

  .drawer-backdrop,
  .drawer-grab,
  .drawer-close,
  .drawer-only,
  .mobile-top {
    display: none;
  }

  aside {
    background: var(--sidebar);
    border-right: 1px solid var(--border);
    padding: 16px 10px 12px;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .brand-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
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
  .scene { display: flex; align-items: center; }
  .scene .del { opacity: 0; }
  .scene:hover .del, .scene .del:focus-visible { opacity: 1; }
  nav form { padding: 2px 0 4px; }

  footer { display: flex; flex-direction: column; gap: 8px; padding: 10px 4px 0 3px; }
  .foot-row { display: flex; align-items: center; justify-content: space-between; gap: 6px; }
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
  .empty {
    grid-column: 1 / -1;
    max-width: 440px;
    margin: 80px auto;
    text-align: center;
    color: var(--text-2);
    line-height: 1.5;
  }
  .empty-title { color: var(--text); font-weight: 550; font-size: 15px; }
  .empty-actions {
    display: flex;
    justify-content: center;
    gap: 8px;
    margin-top: 14px;
    flex-wrap: wrap;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 40;
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
  .dialog h2 { margin: 0; font-size: 16px; font-weight: 600; }
  .dialog-text { margin: -4px 0 0; color: var(--text-2); font-size: 13px; line-height: 1.45; }
  .checks { display: flex; flex-direction: column; gap: 2px; max-height: 260px; overflow-y: auto; }
  .check { display: flex; align-items: center; gap: 10px; padding: 7px 8px; border-radius: 7px; cursor: pointer; }
  .check:hover { background: var(--hover); }
  .check span { flex: 1; }
  .check small { color: var(--text-3); font-size: 12px; }
  .dialog-actions { display: flex; align-items: center; gap: 6px; margin-top: 4px; }
  .spacer { flex: 1; }

  .config-dialog { width: min(640px, 94vw); max-height: 86vh; }
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
  .path-hint { font-size: 11.5px; color: var(--text-3); }
  .config-box {
    margin: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 10px 12px;
    max-height: min(300px, 40vh);
    overflow: auto;
    font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-2);
  }
  .json-editor-wrap {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .json-editor-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .config-editor {
    width: 100%;
    min-height: 210px;
    max-height: min(320px, 42vh);
    resize: vertical;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 10px 12px;
    color: var(--text);
    font-family: ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace;
    font-size: 12px;
    line-height: 1.45;
    outline: none;
  }
  .config-editor:focus { border-color: var(--focus); }
  .json-error {
    margin: 2px 0 0;
    color: #ff6b6b;
    font-size: 12px;
    line-height: 1.4;
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
    z-index: 50;
    transform: translateX(-50%);
    background: var(--surface-2);
    border: 1px solid var(--border);
    padding: 9px 14px;
    border-radius: 10px;
    font-size: 13px;
    box-shadow: 0 10px 30px #0006;
    max-width: min(520px, 90vw);
    animation: fade 0.15s ease-out;
  }
  @keyframes fade { from { opacity: 0; } }

  @media (hover: none) {
    .scene .del { opacity: 0.7; }
  }

  @media (max-width: 680px) {
    .app {
      grid-template-columns: 1fr;
      height: 100dvh;
    }

    .desktop-header { display: none; }

    .drawer-backdrop {
      display: block;
      position: fixed;
      inset: 0;
      z-index: 25;
      background: #0009;
      animation: fade 0.14s ease-out;
    }

    aside {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 30;
      max-height: 82dvh;
      border-right: 0;
      border-top: 1px solid var(--border);
      border-radius: 18px 18px 0 0;
      padding: 8px 12px calc(14px + env(safe-area-inset-bottom, 0px));
      box-shadow: 0 -16px 48px #0009;
      transform: translateY(105%);
      transition: transform 0.22s cubic-bezier(0.2, 0.9, 0.2, 1);
      pointer-events: none;
    }
    aside.open {
      transform: translateY(0);
      pointer-events: auto;
    }
    .drawer-grab {
      display: block;
      width: 36px;
      height: 4px;
      border-radius: 2px;
      background: var(--track-strong);
      margin: 2px auto 8px;
      flex-shrink: 0;
    }
    .brand { padding: 2px 10px 10px; }
    .drawer-close { display: inline-flex; }
    .drawer-only { display: block; }
    .nav-item {
      padding: 10px 10px;
      font-size: 14.5px;
    }

    main {
      padding: 0 max(16px, env(safe-area-inset-right, 0px)) calc(32px + env(safe-area-inset-bottom, 0px)) max(16px, env(safe-area-inset-left, 0px));
    }

    .mobile-top {
      display: flex;
      flex-direction: column;
      gap: 10px;
      position: sticky;
      top: 0;
      z-index: 10;
      background: color-mix(in srgb, var(--bg) 90%, transparent);
      backdrop-filter: blur(14px);
      -webkit-backdrop-filter: blur(14px);
      margin: 0 -16px 14px;
      padding: calc(10px + env(safe-area-inset-top, 0px)) 16px 10px;
      border-bottom: 1px solid var(--border);
    }
    .mobile-bar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 10px;
    }
    .mobile-title {
      display: flex;
      align-items: center;
      gap: 9px;
      min-width: 0;
    }
    .mobile-logo {
      width: 24px;
      height: 24px;
      flex-shrink: 0;
    }
    .mobile-heading { min-width: 0; }
    .mobile-heading h1 {
      font-size: 17px;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .mobile-heading .sub {
      margin-top: 1px;
      font-size: 12px;
    }
    .mobile-actions {
      display: flex;
      align-items: center;
      gap: 5px;
      flex-shrink: 0;
    }
    .mobile-power {
      height: 32px;
      padding: 0 10px;
      font-size: 12.5px;
    }

    .mobile-strips {
      display: flex;
      flex-direction: column;
      gap: 7px;
    }
    .pill-strip {
      display: flex;
      align-items: center;
      gap: 6px;
      overflow-x: auto;
      scrollbar-width: none;
      margin: 0 -16px;
      padding: 0 16px;
    }
    .pill-strip::-webkit-scrollbar { display: none; }
    .scene-strip { padding-top: 1px; }
    .pill {
      height: 30px;
      padding: 0 11px;
      border-radius: 999px;
      font-size: 12.5px;
      white-space: nowrap;
      flex-shrink: 0;
      background: var(--surface);
      border-color: var(--border);
      color: var(--text-2);
    }
    .pill small {
      font-size: 11px;
      color: var(--text-3);
      font-variant-numeric: tabular-nums;
    }
    .pill.sel {
      background: var(--active);
      border-color: var(--border-2);
      color: var(--text);
      font-weight: 550;
    }
    .pill.sel small { color: var(--text-2); }
    .scene-pill :global(svg) { color: var(--accent); }
    .ghost-pill {
      background: transparent;
      border-style: dashed;
      color: var(--text-3);
    }

    .grid { grid-template-columns: 1fr; }
    .empty { margin: 44px auto; }

    .backdrop {
      padding: env(safe-area-inset-top, 12px) 12px env(safe-area-inset-bottom, 12px);
    }
    .config-dialog,
    .log-dialog {
      max-height: calc(100dvh - env(safe-area-inset-top, 12px) - env(safe-area-inset-bottom, 12px) - 16px);
    }
    .toast {
      bottom: calc(18px + env(safe-area-inset-bottom, 0px));
    }
  }
</style>
