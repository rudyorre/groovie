<script lang="ts">
  import { api, fromHex, kelvinToRgb, throttle, toHex, type Action, type Device, type Target } from './api';
  import Icon from './Icon.svelte';
  import Slider from './Slider.svelte';

  let {
    target,
    devices,
    groupName,
  }: {
    target: Target;
    devices: Device[];
    groupName?: string;
  } = $props();

  const PRESETS = ['#ff453a', '#ff9f0a', '#ffd60a', '#30d158', '#64d2ff', '#0a84ff', '#5e5ce6', '#ff375f'];
  const WHITES = [
    { k: 2700, label: 'Warm' },
    { k: 4000, label: 'Neutral' },
    { k: 6500, label: 'Daylight' },
  ];

  const onlineDevices = $derived(devices.filter((d) => d.online && d.state));
  const litDevices = $derived(onlineDevices.filter((d) => d.state!.on));
  const basisDevices = $derived(litDevices.length > 0 ? litDevices : onlineDevices);
  const disabled = $derived(onlineDevices.length === 0);

  const avgBrightness = $derived(
    basisDevices.length > 0
      ? Math.max(
          1,
          Math.min(
            100,
            Math.round(basisDevices.reduce((sum, d) => sum + (d.state?.brightness ?? 0), 0) / basisDevices.length),
          ),
        )
      : 75,
  );

  const whiteDevices = $derived(basisDevices.filter((d) => (d.state?.kelvin ?? 0) > 0));
  const colorDevices = $derived(
    basisDevices.filter((d) => {
      const st = d.state;
      return !!st && st.kelvin === 0 && (st.color.r !== 0 || st.color.g !== 0 || st.color.b !== 0);
    }),
  );

  const inferredMode = $derived<'color' | 'white'>(
    whiteDevices.length > colorDevices.length && whiteDevices.length > 0 ? 'white' : 'color',
  );
  let pickedMode = $state<'color' | 'white' | null>(null);
  const targetKey = $derived(target.type === 'group' ? `group:${target.id}` : 'all');
  $effect(() => {
    void targetKey;
    pickedMode = null;
  });
  const mode = $derived(pickedMode ?? inferredMode);

  const uniformHex = $derived.by(() => {
    if (!basisDevices.length || colorDevices.length !== basisDevices.length) return '';
    const first = toHex(colorDevices[0].state!.color);
    return colorDevices.every((d) => toHex(d.state!.color) === first) ? first : '';
  });
  const customSelected = $derived(uniformHex !== '' && !PRESETS.includes(uniformHex));

  const fallbackColorHex = $derived(
    uniformHex || (colorDevices[0]?.state ? toHex(colorDevices[0].state.color) : '#ff9f0a'),
  );

  const activeKelvin = $derived.by(() => {
    if (!whiteDevices.length) return 4000;
    const avg = whiteDevices.reduce((sum, d) => sum + (d.state?.kelvin ?? 4000), 0) / whiteDevices.length;
    return Math.round(avg / 100) * 100;
  });
  const uniformKelvin = $derived.by(() => {
    if (!basisDevices.length || whiteDevices.length !== basisDevices.length) return 0;
    const first = whiteDevices[0].state!.kelvin;
    return whiteDevices.every((d) => d.state!.kelvin === first) ? first : 0;
  });

  const fillCss = $derived.by(() => {
    if (!litDevices.length) return 'var(--track-strong)';
    if (uniformHex) return uniformHex;
    if (uniformKelvin) {
      const rgb = kelvinToRgb(uniformKelvin);
      return `rgb(${rgb.r} ${rgb.g} ${rgb.b})`;
    }
    return 'var(--accent)';
  });

  const send = (a: Action) => api.then((x) => x.apply(target, a)).catch(console.error);
  const sendThrottled = throttle(send);
</script>

<section
  class="master-bar"
  class:disabled
  aria-label={groupName ? `${groupName} master controls` : 'All lights master controls'}
  style="--master-fill: {fillCss}"
>
  <div class="col bri-col">
    <div class="bar-head">
      <span class="bar-title">{groupName ? `${groupName} master` : 'All lights master'}</span>
      <span class="bar-value"><Icon name="sun" size={13} /> {avgBrightness}%</span>
    </div>
    <div class="bri-track">
      <Slider
        label={groupName ? `${groupName} brightness` : 'All lights brightness'}
        min={1}
        max={100}
        value={avgBrightness}
        {disabled}
        oninput={(v) => sendThrottled({ type: 'brightness', value: v })}
      />
    </div>
  </div>

  <div class="col tone-col">
    <div class="bar-head">
      <div class="segmented compact-seg" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'color'}
          class:sel={mode === 'color'}
          {disabled}
          onclick={() => (pickedMode = 'color')}
        >
          Colour
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'white'}
          class:sel={mode === 'white'}
          {disabled}
          onclick={() => (pickedMode = 'white')}
        >
          White
        </button>
      </div>
      {#if mode === 'white'}
        <span class="bar-value">{activeKelvin}K</span>
      {/if}
    </div>

    {#if mode === 'color'}
      <div class="swatches">
        {#each PRESETS as c (c)}
          <button
            type="button"
            class="swatch"
            class:sel={uniformHex === c}
            style="--c: {c}"
            title={c}
            {disabled}
            aria-label="Set {groupName ?? 'all lights'} colour {c}"
            onclick={() => send({ type: 'color', value: fromHex(c) })}
          ></button>
        {/each}
        <label
          class="swatch custom"
          class:sel={customSelected}
          class:filled={customSelected}
          class:off={disabled}
          style="--c: {uniformHex || fallbackColorHex}"
          title="Custom colour"
        >
          <input
            type="color"
            value={fallbackColorHex}
            {disabled}
            aria-label="Custom colour for {groupName ?? 'all lights'}"
            oninput={(e) => sendThrottled({ type: 'color', value: fromHex(e.currentTarget.value) })}
          />
        </label>
      </div>
    {:else}
      <div class="white-row">
        <div class="kelvin-wrap">
          <Slider
            variant="kelvin"
            label={groupName ? `${groupName} colour temperature` : 'All lights colour temperature'}
            min={2000}
            max={9000}
            step={100}
            value={activeKelvin}
            {disabled}
            oninput={(v) => sendThrottled({ type: 'kelvin', value: v })}
          />
        </div>
        <div class="chips">
          {#each WHITES as w (w.k)}
            <button
              type="button"
              class="chip"
              class:sel={uniformKelvin === w.k}
              {disabled}
              onclick={() => send({ type: 'kelvin', value: w.k })}
            >
              {w.label}
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </div>
</section>

<style>
  .master-bar {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 12px 14px;
    margin-bottom: 16px;
    display: grid;
    grid-template-columns: minmax(220px, 1fr) minmax(280px, 1.35fr);
    gap: 14px 20px;
    align-items: center;
  }
  .master-bar.disabled {
    opacity: 0.6;
  }

  .col {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .bar-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 24px;
  }
  .bar-title {
    font-size: 12px;
    font-weight: 550;
    color: var(--text-2);
    letter-spacing: 0.01em;
  }
  .bar-value {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }

  .bri-track {
    --fill: var(--master-fill);
  }

  .compact-seg {
    width: 156px;
  }
  .compact-seg button {
    height: 22px;
    font-size: 12px;
  }

  .swatches {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    min-height: 32px;
  }
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
  .swatch:hover:not(:disabled) {
    transform: scale(1.08);
  }
  .swatch.sel {
    box-shadow: 0 0 0 2px var(--surface), 0 0 0 4px var(--text);
  }
  .custom {
    position: relative;
    background: conic-gradient(#ff453a, #ffd60a, #30d158, #64d2ff, #5e5ce6, #ff375f, #ff453a);
    overflow: hidden;
  }
  .custom.filled {
    background: var(--c);
  }
  .custom.off {
    cursor: default;
    opacity: 0.5;
  }
  .custom input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: pointer;
    width: 100%;
    height: 100%;
  }
  .custom.off input {
    cursor: default;
  }

  .white-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    min-height: 32px;
  }
  .kelvin-wrap {
    flex: 1 1 140px;
    min-width: 120px;
  }
  .chips {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  @media (max-width: 860px) {
    .master-bar {
      grid-template-columns: 1fr;
      gap: 12px;
    }
  }
</style>
