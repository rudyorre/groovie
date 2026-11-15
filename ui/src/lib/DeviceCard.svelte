<script lang="ts">
  import { api, fromHex, kelvinToRgb, throttle, toHex, type Action, type Device } from './api';
  import Icon, { type IconName } from './Icon.svelte';
  import Slider from './Slider.svelte';

  let { device }: { device: Device } = $props();

  const PRESETS = ['#ff453a', '#ff9f0a', '#ffd60a', '#30d158', '#64d2ff', '#0a84ff', '#5e5ce6', '#ff375f'];
  const WHITES = [
    { k: 2700, label: 'Warm' },
    { k: 4000, label: 'Neutral' },
    { k: 6500, label: 'Daylight' },
  ];

  const s = $derived(device.state);
  const on = $derived(!!s?.on && device.online);
  const lit = $derived(s ? (s.kelvin ? kelvinToRgb(s.kelvin) : s.color) : null);
  const litCss = $derived(lit ? `rgb(${lit.r} ${lit.g} ${lit.b})` : 'var(--track)');
  // Dark icon on light colours, white icon on dark ones.
  const iconInk = $derived(lit && 0.2126 * lit.r + 0.7152 * lit.g + 0.0722 * lit.b < 140 ? '#fff' : '#111');
  const hex = $derived(s && !s.kelvin ? toHex(s.color) : '');
  const custom = $derived(hex !== '' && !PRESETS.includes(hex));

  // Which panel is showing; follows the light's real mode unless the user just switched.
  let picked = $state<'color' | 'white' | null>(null);
  const mode = $derived(picked ?? (s?.kelvin ? 'white' : 'color'));

  const icon: IconName = $derived.by(() => {
    const sku = device.sku.toUpperCase();
    if (['H6072', 'H6076', 'H6079', 'H607C'].includes(sku)) return 'lamp';
    if (['H6046', 'H6047', 'H6056', 'H6043'].includes(sku)) return 'bars';
    return 'bulb';
  });

  const status = $derived(
    !device.online ? 'Offline' : !s ? 'Connecting…' : s.on ? `On · ${s.brightness}%` : 'Off',
  );

  const target = $derived({ type: 'device' as const, id: device.id });
  const send = (a: Action) => api.then((x) => x.apply(target, a)).catch(console.error);
  const sendThrottled = throttle(send);

  function setMode(m: 'color' | 'white') {
    picked = m;
    if (!s) return;
    if (m === 'white' && !s.kelvin) send({ type: 'kelvin', value: 4000 });
    if (m === 'color' && s.kelvin) send({ type: 'color', value: s.color });
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
</script>

<article class="card" class:on class:offline={!device.online} style="--lit: {litCss}; --ink: {iconInk}">
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
            onclick={() => { picked = 'color'; send({ type: 'color', value: fromHex(c) }); }}
          ></button>
        {/each}
        <label class="swatch custom" class:sel={custom} class:filled={custom} style="--c: {hex}" title="Custom colour">
          <input
            type="color"
            value={hex || '#ffffff'}
            oninput={(e) => { picked = 'color'; sendThrottled({ type: 'color', value: fromHex(e.currentTarget.value) }); }}
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
          value={s.kelvin || 4000}
          oninput={(v) => sendThrottled({ type: 'kelvin', value: v })}
        />
        <div class="chips">
          {#each WHITES as w (w.k)}
            <button class="chip" class:sel={s.kelvin === w.k} onclick={() => send({ type: 'kelvin', value: w.k })}>
              {w.label}
            </button>
          {/each}
          <span class="k">{s.kelvin ? `${s.kelvin}K` : ''}</span>
        </div>
      </div>
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
</style>
