<script lang="ts">
  import { onMount } from 'svelte';
  import { api, isTauri, type Device, type Group, type Scene, type Target, type Update } from './lib/api';
  import DeviceCard from './lib/DeviceCard.svelte';
  import Icon from './lib/Icon.svelte';

  let devices = $state<Device[]>([]);
  let groups = $state<Group[]>([]);
  let scenes = $state<Scene[]>([]);
  /** 'all' or a group id */
  let view = $state('all');
  let toast = $state('');
  let version = $state('');
  let update = $state<Update | null>(null);
  let updating = $state(false);

  // Dialogs / inline editors
  let editing = $state<Group | null>(null); // id '' = new group
  let addingLight = $state(false);
  let ipDraft = $state('');
  let namingScene = $state(false);
  let sceneDraft = $state('');
  let scanning = $state(false);

  const group = $derived(groups.find((g) => g.id === view));
  const shown = $derived(group ? devices.filter((d) => group.members.includes(d.id)) : devices);
  const target = $derived<Target>(group ? { type: 'group', id: group.id } : { type: 'all' });
  const onCount = $derived(shown.filter((d) => d.online && d.state?.on).length);

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
    await run(async (a) => {
      [devices, groups, scenes] = await Promise.all([a.listDevices(), a.listGroups(), a.listScenes()]);
    });
    if (view !== 'all' && !groups.some((g) => g.id === view)) view = 'all';
  }

  onMount(() => {
    reload();
    let off = () => {};
    api.then(async (a) => {
      off = a.onEvent((e) => {
        if (e.type === 'deviceUpdated') {
          const i = devices.findIndex((d) => d.id === e.device.id);
          if (i >= 0) devices[i] = e.device;
          else devices = [...devices, e.device].sort((x, y) => x.name.localeCompare(y.name));
        } else reload();
      });
      version = await a.appVersion().catch(() => '');
      update = await a.checkUpdate().catch(() => null);
    });
    return () => off();
  });

  async function rescan() {
    scanning = true;
    await run((a) => a.discover());
    setTimeout(() => (scanning = false), 1200);
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
    sceneDraft = '';
    if (name) await run((a) => a.saveScene(name));
  }

  async function addLight() {
    const ip = ipDraft.trim();
    addingLight = false;
    ipDraft = '';
    if (ip) await run((a) => a.addIp(ip));
  }

  async function installUpdate() {
    if (!update) return;
    updating = true;
    try {
      await update.install();
    } catch (e) {
      updating = false;
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
          <button class="nav-item" title="Apply scene" onclick={() => run((a) => a.applyScene(sc.id))}>
            <Icon name="sparkle" /> <span>{sc.name}</span>
          </button>
          <button class="ghost icon-btn del" title="Delete scene" onclick={() => run((a) => a.deleteScene(sc.id))}>
            <Icon name="x" size={14} />
          </button>
        </div>
      {:else}
        {#if !namingScene}<p class="empty-hint">Save the current look with +.</p>{/if}
      {/each}
    </nav>

    <footer>
      {#if update}
        <button class="update" onclick={installUpdate} disabled={updating}>
          <Icon name="download" size={14} />
          {updating ? 'Updating…' : `Update to ${update.version}`}
        </button>
      {/if}
      <span class="version">{isTauri ? `v${version}` : 'Demo mode · mock lights'}</span>
    </footer>
  </aside>

  <main>
    <header>
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

    <section class="grid">
      {#each shown as d (d.id)}
        <DeviceCard device={d} />
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

{#if addingLight}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (addingLight = false)}>
    <form class="dialog" onsubmit={(e) => { e.preventDefault(); addLight(); }}>
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
{/if}

{#if toast}
  <div class="toast" role="status">{toast}</div>
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

  footer { display: flex; flex-direction: column; gap: 8px; padding: 10px 10px 0; }
  .version { font-size: 12px; color: var(--text-3); }
  .update {
    justify-content: center;
    background: var(--accent);
    border-color: var(--accent);
    color: #1a1204;
    font-weight: 550;
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
  .dialog h2 { margin: 0; font-size: 16px; font-weight: 600; }
  .dialog-text { margin: -4px 0 0; color: var(--text-2); font-size: 13px; line-height: 1.45; }
  .checks { display: flex; flex-direction: column; gap: 2px; max-height: 260px; overflow-y: auto; }
  .check { display: flex; align-items: center; gap: 10px; padding: 7px 8px; border-radius: 7px; cursor: pointer; }
  .check:hover { background: var(--hover); }
  .check span { flex: 1; }
  .check small { color: var(--text-3); font-size: 12px; }
  .dialog-actions { display: flex; gap: 6px; margin-top: 4px; }
  .spacer { flex: 1; }

  .toast {
    position: fixed;
    bottom: 18px;
    left: 50%;
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

  @media (max-width: 680px) {
    .app { grid-template-columns: 1fr; height: auto; }
    aside { border-right: 0; border-bottom: 1px solid var(--border); }
    main { padding: 18px 16px 32px; }
  }
</style>
