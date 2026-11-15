<script lang="ts">
  import { api, fromHex, kelvinToRgb, throttle, toHex, type Action, type Device } from './api';

  let { device }: { device: Device } = $props();

  let expanded = $state(false);
  let renaming = $state(false);
  let newName = $state('');

  const s = $derived(device.state);
  const glow = $derived(s ? (s.kelvin ? kelvinToRgb(s.kelvin) : s.color) : null);
  const glowCss = $derived(glow && s?.on ? `rgb(${glow.r} ${glow.g} ${glow.b})` : 'transparent');

  const target = $derived({ type: 'device' as const, id: device.id });
  const send = (a: Action) => api.then((x) => x.apply(target, a)).catch(console.error);
  const sendThrottled = throttle(send);

  function saveName() {
    renaming = false;
    api.then((x) => x.rename(device.id, newName.trim()));
  }
</script>

<article class="card" class:off={!s?.on} class:offline={!device.online} style="--glow: {glowCss}">
  <header>
    <button class="title" onclick={() => (expanded = !expanded)} aria-expanded={expanded}>
      <span class="swatch" style="background: {glowCss}"></span>
      <span>
        <strong>{device.name}</strong>
        <small>{device.sku} · {device.online ? device.ip : 'offline'}</small>
      </span>
    </button>
    <label class="switch" title="Power">
      <input
        type="checkbox"
        checked={s?.on ?? false}
        disabled={!device.online}
        onchange={(e) => send({ type: 'power', value: e.currentTarget.checked })}
      />
      <span></span>
    </label>
  </header>

  {#if s}
    <label class="row">
      <span>☀</span>
      <input
        type="range"
        min="1"
        max="100"
        value={s.brightness}
        disabled={!device.online}
        oninput={(e) => sendThrottled({ type: 'brightness', value: +e.currentTarget.value })}
      />
      <span class="num">{s.brightness}%</span>
    </label>
  {/if}

  {#if expanded && s}
    <div class="more">
      <label class="row">
        <span>Colour</span>
        <input
          type="color"
          value={toHex(s.color)}
          oninput={(e) => sendThrottled({ type: 'color', value: fromHex(e.currentTarget.value) })}
        />
      </label>
      <label class="row">
        <span>White</span>
        <input
          class="kelvin"
          type="range"
          min="2000"
          max="9000"
          step="100"
          value={s.kelvin || 4000}
          oninput={(e) => sendThrottled({ type: 'kelvin', value: +e.currentTarget.value })}
        />
        <span class="num">{s.kelvin ? `${s.kelvin}K` : '–'}</span>
      </label>
      {#if renaming}
        <form class="row" onsubmit={(e) => { e.preventDefault(); saveName(); }}>
          <!-- svelte-ignore a11y_autofocus -->
          <input class="text" bind:value={newName} placeholder={device.sku} autofocus />
          <button type="submit">Save</button>
        </form>
      {:else}
        <button class="link" onclick={() => { newName = device.name; renaming = true; }}>Rename</button>
      {/if}
    </div>
  {/if}
</article>

<style>
  .card {
    position: relative;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 16px;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    box-shadow: 0 0 40px -12px var(--glow);
    transition: box-shadow 0.3s, opacity 0.2s;
  }
  .card.off { box-shadow: none; }
  .card.offline { opacity: 0.5; }
  header { display: flex; align-items: center; gap: 12px; }
  .title {
    all: unset;
    cursor: pointer;
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }
  .title > span:last-child { display: flex; flex-direction: column; min-width: 0; }
  strong { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  small { color: var(--muted); font-size: 12px; }
  .swatch {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    border: 2px solid var(--border);
    flex: none;
  }
  .row { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--muted); }
  .row input[type='range'] { flex: 1; }
  .num { width: 48px; text-align: right; font-variant-numeric: tabular-nums; }
  .more { display: flex; flex-direction: column; gap: 10px; border-top: 1px solid var(--border); padding-top: 10px; }
  .more .row > span:first-child { width: 48px; }
  .kelvin { accent-color: #ffcf8a; }
  .link { align-self: flex-start; }
</style>
