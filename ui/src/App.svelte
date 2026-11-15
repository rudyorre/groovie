<script lang="ts">
  import { onMount } from 'svelte';
  import { api, isTauri, throttle, type Device, type Group, type Scene, type Target } from './lib/api';
  import DeviceCard from './lib/DeviceCard.svelte';

  let devices = $state<Device[]>([]);
  let groups = $state<Group[]>([]);
  let scenes = $state<Scene[]>([]);
  /** 'all' or a group id */
  let view = $state('all');
  let error = $state('');

  // Group editor: null = closed; id '' = creating a new group.
  let editing = $state<Group | null>(null);
  let addIp = $state('');

  const group = $derived(groups.find((g) => g.id === view));
  const shown = $derived(group ? devices.filter((d) => group.members.includes(d.id)) : devices);
  const target = $derived<Target>(group ? { type: 'group', id: group.id } : { type: 'all' });
  const anyOn = $derived(shown.some((d) => d.state?.on));

  async function run<T>(f: (a: Awaited<typeof api>) => Promise<T>) {
    try {
      error = '';
      return await f(await api);
    } catch (e) {
      error = String(e);
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
    api.then((a) => {
      off = a.onEvent((e) => {
        if (e.type === 'deviceUpdated') {
          const i = devices.findIndex((d) => d.id === e.device.id);
          if (i >= 0) devices[i] = e.device;
          else devices = [...devices, e.device].sort((x, y) => x.name.localeCompare(y.name));
        } else reload();
      });
    });
    return () => off();
  });

  const brightnessAll = throttle((v: number) => run((a) => a.apply(target, { type: 'brightness', value: v })));

  async function saveGroup() {
    const g = editing!;
    editing = null;
    if (!g.name.trim()) return;
    if (g.id) await run((a) => a.updateGroup(g));
    else {
      const created = await run((a) => a.createGroup(g.name.trim(), g.members));
      if (created) view = created.id;
    }
  }

  async function saveScene() {
    const name = prompt('Scene name', 'Evening');
    if (name?.trim()) await run((a) => a.saveScene(name.trim()));
  }
</script>

<div class="app">
  <aside>
    <h1>Groovie</h1>

    <nav>
      <button class:active={view === 'all'} onclick={() => (view = 'all')}>All lights</button>
      <h2>
        Groups
        <button class="icon" title="New group" onclick={() => (editing = { id: '', name: '', members: [] })}>+</button>
      </h2>
      {#each groups as g (g.id)}
        <button class:active={view === g.id} onclick={() => (view = g.id)}>{g.name}</button>
      {:else}
        <p class="hint">No groups yet</p>
      {/each}

      <h2>
        Scenes
        <button class="icon" title="Save current look as a scene" onclick={saveScene}>+</button>
      </h2>
      {#each scenes as s (s.id)}
        <div class="scene">
          <button onclick={() => run((a) => a.applyScene(s.id))}>{s.name}</button>
          <button class="icon" title="Delete scene" onclick={() => run((a) => a.deleteScene(s.id))}>×</button>
        </div>
      {:else}
        <p class="hint">Save the current look with +</p>
      {/each}
    </nav>

    <form class="addip" onsubmit={(e) => { e.preventDefault(); run((a) => a.addIp(addIp.trim())); addIp = ''; }}>
      <input class="text" bind:value={addIp} placeholder="Add light by IP" />
    </form>
    {#if !isTauri}<p class="hint">Demo mode (mock lights)</p>{/if}
  </aside>

  <main>
    <header>
      <h2>{group?.name ?? 'All lights'}</h2>
      {#if group}
        <button class="link" onclick={() => (editing = structuredClone($state.snapshot(group)))}>Edit</button>
      {/if}
      <span class="spacer"></span>
      <input
        type="range"
        min="1"
        max="100"
        title="Brightness for all shown lights"
        oninput={(e) => brightnessAll(+e.currentTarget.value)}
      />
      <button onclick={() => run((a) => a.apply(target, { type: 'power', value: !anyOn }))}>
        {anyOn ? 'Turn off' : 'Turn on'}
      </button>
      <button title="Search for lights" onclick={() => run((a) => a.discover())}>↻</button>
    </header>

    {#if error}<p class="error">{error}</p>{/if}

    <section class="grid">
      {#each shown as d (d.id)}
        <DeviceCard device={d} />
      {:else}
        <div class="empty">
          {#if group}
            This group is empty. Click <b>Edit</b> to add lights.
          {:else}
            <p><b>Looking for lights…</b></p>
            <p>Make sure each light has <b>LAN Control</b> turned on in the Govee app (device settings), and that
              this computer is on the same Wi-Fi network.</p>
          {/if}
        </div>
      {/each}
    </section>
  </main>
</div>

{#if editing}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (editing = null)}>
    <form class="modal" onsubmit={(e) => { e.preventDefault(); saveGroup(); }}>
      <h2>{editing.id ? 'Edit group' : 'New group'}</h2>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="text" bind:value={editing.name} placeholder="Group name" autofocus />
      {#each devices as d (d.id)}
        <label class="check">
          <input type="checkbox" value={d.id} bind:group={editing.members} />
          {d.name} <small>{d.sku}</small>
        </label>
      {/each}
      <div class="actions">
        {#if editing.id}
          <button
            type="button"
            class="danger"
            onclick={() => { const id = editing!.id; editing = null; run((a) => a.deleteGroup(id)); }}
          >Delete</button>
        {/if}
        <span class="spacer"></span>
        <button type="button" onclick={() => (editing = null)}>Cancel</button>
        <button type="submit" class="primary">Save</button>
      </div>
    </form>
  </div>
{/if}

<style>
  .app { display: grid; grid-template-columns: 220px 1fr; height: 100vh; }
  aside {
    border-right: 1px solid var(--border);
    padding: 18px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow-y: auto;
  }
  h1 {
    font-size: 22px;
    margin: 0 0 8px 8px;
    background: linear-gradient(90deg, #ff8a3d, #c86bff, #4fc3ff);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  nav { display: flex; flex-direction: column; gap: 2px; flex: 1; }
  nav h2 {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    margin: 16px 8px 4px;
  }
  nav button:not(.icon) { all: unset; cursor: pointer; padding: 7px 10px; border-radius: 8px; flex: 1; }
  nav button:not(.icon):hover { background: var(--hover); }
  nav button.active { background: var(--hover); font-weight: 600; }
  .scene { display: flex; align-items: center; }
  .hint { color: var(--muted); font-size: 12px; margin: 2px 10px; }
  main { overflow-y: auto; padding: 18px 24px; }
  main > header { display: flex; align-items: center; gap: 12px; margin-bottom: 18px; }
  main > header h2 { margin: 0; font-size: 20px; }
  .spacer { flex: 1; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 16px; }
  .empty { grid-column: 1 / -1; color: var(--muted); text-align: center; padding: 60px 20px; max-width: 480px; margin: auto; }
  .error { color: #ff7b7b; font-size: 13px; }
  .backdrop { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; }
  .modal {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 16px;
    padding: 20px;
    width: min(360px, 90vw);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .modal h2 { margin: 0 0 4px; font-size: 17px; }
  .check { display: flex; gap: 8px; align-items: center; }
  .check small { color: var(--muted); }
  .actions { display: flex; gap: 8px; margin-top: 8px; }
  @media (max-width: 640px) {
    .app { grid-template-columns: 1fr; height: auto; }
    aside { border-right: 0; border-bottom: 1px solid var(--border); }
  }
</style>
