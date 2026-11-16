<script module lang="ts">
  import type { Rgb } from './api';

  type ModeMemory = { color?: Rgb; kelvin?: number };
  const STORAGE_KEY = 'groovie:mode-memory:v1';

  function loadAllMemory(): Record<string, ModeMemory> {
    try {
      const raw = typeof localStorage !== 'undefined' ? localStorage.getItem(STORAGE_KEY) : null;
      return raw ? (JSON.parse(raw) as Record<string, ModeMemory>) : {};
    } catch {
      return {};
    }
  }

  function readMemory(id: string): ModeMemory {
    return loadAllMemory()[id] ?? {};
  }

  function writeMemory(id: string, patch: ModeMemory): ModeMemory {
    const all = loadAllMemory();
    const prev = all[id] ?? {};
    const next: ModeMemory = {
      ...(prev.color ? { color: prev.color } : {}),
      ...(prev.kelvin ? { kelvin: prev.kelvin } : {}),
      ...(patch.color ? { color: patch.color } : {}),
      ...(patch.kelvin ? { kelvin: patch.kelvin } : {}),
    };
    if (
      prev.kelvin === next.kelvin &&
      prev.color?.r === next.color?.r &&
      prev.color?.g === next.color?.g &&
      prev.color?.b === next.color?.b
    ) {
      return next;
    }
    all[id] = next;
    try {
      if (typeof localStorage !== 'undefined') {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(all));
      }
    } catch {
      // Ignore storage quota or restricted webview storage errors.
    }
    return next;
  }
</script>

<script lang="ts">
  import { api, fromHex, kelvinToRgb, throttle, toHex, type Action, type Device } from './api';
  import Icon, { type IconName } from './Icon.svelte';
  import Slider from './Slider.svelte';

  let {
    device,
    syncing = false,
    onOpenSync,
    onForget,
  }: {
    device: Device;
    syncing?: boolean;
    onOpenSync?: () => void;
    onForget?: (device: Device) => void;
  } = $props();

  const PRESETS = ['#ff453a', '#ff9f0a', '#ffd60a', '#30d158', '#64d2ff', '#0a84ff', '#5e5ce6', '#ff375f'];
  const WHITES = [
    { k: 2700, label: 'Warm' },
    { k: 4000, label: 'Neutral' },
    { k: 6500, label: 'Daylight' },
  ];

  const s = $derived(device.state);
  const on = $derived(!!s?.on && device.online);
  // Govee lights report RGB (0,0,0) + kelvin 0 when playing a built-in scene or music mode.
  const isEffect = $derived(!!s && s.kelvin === 0 && s.color.r === 0 && s.color.g === 0 && s.color.b === 0);
  const lit = $derived(
    s
      ? syncing
        ? { r: 255, g: 176, b: 32 }
        : s.kelvin
          ? kelvinToRgb(s.kelvin)
          : isEffect
            ? { r: 255, g: 176, b: 32 }
            : s.color
      : null,
  );
  const litCss = $derived(lit ? `rgb(${lit.r} ${lit.g} ${lit.b})` : 'var(--track)');
  // Dark icon on light colours, white icon on dark ones.
  const iconInk = $derived(lit && 0.2126 * lit.r + 0.7152 * lit.g + 0.0722 * lit.b < 140 ? '#fff' : '#111');
  const hex = $derived(s && !s.kelvin && !isEffect ? toHex(s.color) : '');
  const custom = $derived(hex !== '' && !PRESETS.includes(hex));

  // Remember each light's last RGB colour and Kelvin across tab switches and restarts.
  let saved = $state<ModeMemory>({});
  $effect(() => {
    const id = device.id;
    const st = device.state;
    const color =
      device.lastColor ??
      (st && st.kelvin === 0 && (st.color.r !== 0 || st.color.g !== 0 || st.color.b !== 0) ? st.color : undefined);
    const kelvin = device.lastKelvin ?? (st && st.kelvin > 0 ? st.kelvin : undefined);
    saved = color || kelvin ? writeMemory(id, { color, kelvin }) : readMemory(id);
  });

  const rememberedColor = $derived<Rgb>(device.lastColor ?? saved.color ?? { r: 255, g: 159, b: 10 });
  const rememberedKelvin = $derived<number>(device.lastKelvin ?? saved.kelvin ?? 4000);
  const activeKelvin = $derived<number>(s?.kelvin || rememberedKelvin);

  // Follow the light's real mode, including external changes from the Govee app.
  const actualMode = $derived<'color' | 'white'>(s?.kelvin ? 'white' : 'color');
  let picked = $state<'color' | 'white' | null>(null);
  const mode = $derived(picked ?? actualMode);
  $effect(() => {
    void actualMode;
    picked = null;
  });

  const icon: IconName = $derived.by(() => {
    const sku = device.sku.toUpperCase();
    if (['H6072', 'H6076', 'H6079', 'H607C'].includes(sku)) return 'lamp';
    if (['H6046', 'H6047', 'H6056', 'H6043'].includes(sku)) return 'bars';
    return 'bulb';
  });

  const status = $derived(
    !device.online
      ? 'Offline'
      : !s
        ? 'Connecting…'
        : s.on
          ? `On · ${s.brightness}%${syncing ? ' · Syncing' : isEffect ? ' · Scene' : ''}`
          : 'Off',
  );

  const target = $derived({ type: 'device' as const, id: device.id });
  const send = (a: Action) => api.then((x) => x.apply(target, a)).catch(console.error);
  const sendThrottled = throttle(send);

  function setMode(m: 'color' | 'white') {
    if (m === mode) return;
    picked = m;
    if (!s) return;
    if (m === 'white' && !s.kelvin) send({ type: 'kelvin', value: rememberedKelvin });
    if (m === 'color' && s.kelvin) send({ type: 'color', value: rememberedColor });
  }

  // Inline rename
  let editing = $state(false);
  let draft = $state('');
  function startRename() {
    draft = device.name;
    editing = true;
  }
  function finishRename(save: boolean) {
    if (!editing) return;
    editing = false;
    if (save && draft.trim() !== device.name) api.then((x) => x.rename(device.id, draft.trim()));
  }

  // Per-card details & two-step forget
  let detailsOpen = $state(false);
  let confirmingForget = $state(false);
  let copiedDetails = $state(false);

  async function copyDetails() {
    const lines = [
      `${device.name} (${device.sku})`,
      `IP: ${device.ip}`,
      `ID: ${device.id}`,
      ...(device.firmware ? [`Firmware: ${device.firmware}`] : []),
    ];
    try {
      await navigator.clipboard.writeText(lines.join('\n'));
      copiedDetails = true;
      setTimeout(() => (copiedDetails = false), 1800);
    } catch {
      // Ignore restricted clipboard errors.
    }
  }
</script>

<article
  class="card"
  class:on
  class:offline={!device.online}
  class:details-open={detailsOpen}
  style="--lit: {litCss}; --ink: {iconInk}"
>
  <div class="top">
    <div class="tile"><Icon name={icon} size={20} /></div>
    <div class="meta">
      {#if editing}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="rename"
          bind:value={draft}
          placeholder={device.sku}
          autofocus
          onblur={() => finishRename(true)}
          onkeydown={(e) => {
            if (e.key === 'Enter') finishRename(true);
            if (e.key === 'Escape') finishRename(false);
          }}
        />
      {:else}
        <div class="name">
          <span>{device.name}</span>
          <button class="ghost icon-btn edit" title="Rename" onclick={startRename}><Icon name="pencil" size={13} /></button>
        </div>
      {/if}
      <div class="status">{status}</div>
    </div>
    <button
      type="button"
      class="ghost icon-btn details-btn"
      class:open={detailsOpen}
      title="Light details"
      aria-label="Light details for {device.name}"
      aria-expanded={detailsOpen}
      onclick={() => {
        detailsOpen = !detailsOpen;
        confirmingForget = false;
      }}
    >
      <Icon name="info" size={14} />
    </button>
    <label class="switch" title={on ? 'Turn off' : 'Turn on'}>
      <input
        type="checkbox"
        checked={s?.on ?? false}
        disabled={!device.online}
        onchange={(e) => send({ type: 'power', value: e.currentTarget.checked })}
      />
      <span></span>
    </label>
  </div>

  {#if detailsOpen}
    <div class="details">
      <dl class="details-grid">
        <div>
          <dt>Model</dt>
          <dd>{device.sku}</dd>
        </div>
        <div>
          <dt>IP address</dt>
          <dd>{device.ip}</dd>
        </div>
        <div class="wide">
          <dt>Device ID</dt>
          <dd>{device.id}</dd>
        </div>
        {#if device.firmware}
          <div class="wide">
            <dt>Firmware</dt>
            <dd>{device.firmware}</dd>
          </div>
        {/if}
      </dl>
      <div class="details-actions">
        {#if confirmingForget}
          <span class="confirm-note">Remove from groups, scenes &amp; sync?</span>
          <span class="spacer"></span>
          <button type="button" class="ghost sm" onclick={() => (confirmingForget = false)}>Cancel</button>
          <button
            type="button"
            class="danger-solid sm"
            onclick={() => {
              confirmingForget = false;
              detailsOpen = false;
              onForget?.(device);
            }}
          >
            Forget light
          </button>
        {:else}
          <button type="button" class="ghost sm" onclick={copyDetails}>
            <Icon name={copiedDetails ? 'check' : 'copy'} size={13} />
            {copiedDetails ? 'Copied' : 'Copy'}
          </button>
          <button
            type="button"
            class="ghost sm"
            onclick={() => {
              detailsOpen = false;
              startRename();
            }}
          >
            <Icon name="pencil" size={13} /> Rename
          </button>
          <span class="spacer"></span>
          {#if onForget}
            <button type="button" class="ghost sm danger-text" onclick={() => (confirmingForget = true)}>
              <Icon name="trash" size={13} /> Forget
            </button>
          {/if}
        {/if}
      </div>
    </div>
  {/if}

  {#if s}
    <div class="bri">
      <Slider
        label="Brightness"
        min={1}
        max={100}
        value={s.brightness}
        disabled={!device.online}
        oninput={(v) => sendThrottled({ type: 'brightness', value: v })}
      />
      <span class="bri-label"><Icon name="sun" size={14} /> {s.brightness}%</span>
    </div>

    {#if syncing}
      <button type="button" class="sync-pill" onclick={() => onOpenSync?.()}>
        <span class="sync-dot"></span>
        <Icon name="monitor" size={14} />
        <span>Syncing with screen</span>
      </button>
    {:else}
      <div class="segmented" role="tablist">
        <button role="tab" aria-selected={mode === 'color'} class:sel={mode === 'color'} onclick={() => setMode('color')}>Colour</button>
        <button role="tab" aria-selected={mode === 'white'} class:sel={mode === 'white'} onclick={() => setMode('white')}>White</button>
      </div>

      {#if mode === 'color'}
        <div class="swatches">
          {#each PRESETS as c (c)}
            <button
              class="swatch"
              class:sel={hex === c}
              style="--c: {c}"
              title={c}
              aria-label="Set colour {c}"
              onclick={() => send({ type: 'color', value: fromHex(c) })}
            ></button>
          {/each}
          <label class="swatch custom" class:sel={custom} class:filled={custom} style="--c: {hex}" title="Custom colour">
            <input
              type="color"
              value={hex || toHex(rememberedColor)}
              oninput={(e) => sendThrottled({ type: 'color', value: fromHex(e.currentTarget.value) })}
            />
          </label>
        </div>
      {:else}
        <div class="white">
          <Slider
            variant="kelvin"
            label="Colour temperature"
            min={2000}
            max={9000}
            step={100}
            value={activeKelvin}
            oninput={(v) => sendThrottled({ type: 'kelvin', value: v })}
          />
          <div class="chips">
            {#each WHITES as w (w.k)}
              <button class="chip" class:sel={activeKelvin === w.k} onclick={() => send({ type: 'kelvin', value: w.k })}>
                {w.label}
              </button>
            {/each}
            <span class="k">{activeKelvin}K</span>
          </div>
        </div>
      {/if}
    {/if}
  {/if}
</article>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    transition: border-color 0.2s;
  }
  .card.offline { opacity: 0.55; }
  .card.offline.details-open { opacity: 0.92; }

  .top { display: flex; align-items: center; gap: 12px; }
  .tile {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 10px;
    display: grid;
    place-items: center;
    background: var(--track);
    color: var(--text-2);
    transition: background 0.2s, color 0.2s;
  }
  .on .tile { background: var(--lit); color: var(--ink); }

  .meta { flex: 1; min-width: 0; }
  .name { display: flex; align-items: center; gap: 4px; font-weight: 550; }
  .name span { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .edit { opacity: 0; }
  .card:hover .edit, .edit:focus-visible { opacity: 1; }
  .details-btn { opacity: 0; flex: none; }
  .card:hover .details-btn,
  .card.offline .details-btn,
  .details-btn:focus-visible,
  .details-btn.open {
    opacity: 1;
  }
  .details-btn.open {
    background: var(--surface-2);
    color: var(--text);
  }
  .status { color: var(--text-2); font-size: 12.5px; margin-top: 1px; }
  .rename {
    font: inherit;
    font-weight: 550;
    color: inherit;
    width: 100%;
    background: var(--bg);
    border: 1px solid var(--focus);
    border-radius: 6px;
    padding: 1px 6px;
    margin-left: -7px;
  }

  .details {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 9px;
  }
  .details-grid {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 6px 12px;
  }
  .details-grid .wide {
    grid-column: 1 / -1;
  }
  .details-grid dt {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-3);
  }
  .details-grid dd {
    margin: 1px 0 0;
    font-size: 12px;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .details-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }
  .details-actions .spacer {
    flex: 1;
  }
  .details-actions .sm {
    height: 27px;
    padding: 0 8px;
    font-size: 12px;
    gap: 5px;
  }
  .danger-text {
    color: #ff453a;
  }
  .danger-text:hover:not(:disabled) {
    color: #ff6961;
    background: #ff453a18;
  }
  .danger-solid {
    background: #ff453a;
    border-color: transparent;
    color: #fff;
    font-weight: 550;
  }
  .danger-solid:hover:not(:disabled) {
    background: #ff5b51;
  }
  .confirm-note {
    font-size: 11.5px;
    color: var(--text-2);
  }

  .bri { position: relative; --fill: var(--track-strong); }
  .on .bri { --fill: var(--lit); }
  .bri-label {
    position: absolute;
    right: 10px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
    pointer-events: none;
    mix-blend-mode: difference;
    filter: grayscale(1);
  }

  .swatches { display: flex; gap: 8px; flex-wrap: wrap; }
  .swatch {
    width: 26px;
    height: 26px;
    padding: 0;
    border-radius: 50%;
    border: 0;
    background: var(--c);
    cursor: pointer;
    box-shadow: inset 0 0 0 1px #ffffff1a;
    transition: transform 0.1s;
  }
  .swatch:hover { transform: scale(1.08); }
  .swatch.sel { box-shadow: 0 0 0 2px var(--surface), 0 0 0 4px var(--text); }
  .custom {
    position: relative;
    background: conic-gradient(#ff453a, #ffd60a, #30d158, #64d2ff, #5e5ce6, #ff375f, #ff453a);
    overflow: hidden;
  }
  .custom.filled { background: var(--c); }
  .custom input { position: absolute; inset: 0; opacity: 0; cursor: pointer; width: 100%; height: 100%; }

  .white { display: flex; flex-direction: column; gap: 10px; }
  .chips { display: flex; gap: 6px; align-items: center; }
  .k { margin-left: auto; color: var(--text-2); font-size: 12px; font-variant-numeric: tabular-nums; }

  .sync-pill {
    width: 100%;
    height: 36px;
    justify-content: center;
    gap: 8px;
    border-radius: 9px;
    background: #ffb02016;
    border-color: #ffb02048;
    color: var(--accent);
    font-size: 12.5px;
    font-weight: 550;
  }
  .sync-pill:hover:not(:disabled) {
    background: #ffb02026;
  }
  .sync-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 8px var(--accent);
  }
</style>
