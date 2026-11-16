<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    computeSampleRect,
    throttle,
    type Device,
    type DisplayInfo,
    type Orientation,
    type PlacementMode,
    type Rgb,
    type SampleRect,
    type SyncConfig,
    type SyncPlacement,
    type SyncPreset,
    type SyncPreview,
    type SyncState,
  } from './api';
  import Icon, { type IconName } from './Icon.svelte';
  import Slider from './Slider.svelte';

  let {
    devices,
    syncState,
    preview,
    onToggleSync,
    onUpdateConfig,
    onIdentify,
  }: {
    devices: Device[];
    syncState: SyncState;
    preview: SyncPreview | null;
    onToggleSync: (enabled: boolean, force?: boolean) => void;
    onUpdateConfig: (cfg: SyncConfig) => void;
    onIdentify: (placementId: string) => void;
  } = $props();

  // Monitor box geometry within `.desk-canvas` (`[0..1] × [0..1]` maps to monitor bezel).
  const MX0 = 0.235;
  const MW = 0.53;
  const MY0 = 0.22;
  const MH = 0.56;

  let displays = $state<DisplayInfo[]>([]);
  let selectedId = $state<string | null>(null);
  let identifyingId = $state<string | null>(null);
  let canvasEl = $state<HTMLElement | null>(null);
  let dragState = $state<{ id: string; x: number; y: number } | null>(null);

  const cfg = $derived(syncState.config);
  const deviceMap = $derived(new Map(devices.map((d) => [d.id, d])));

  const placements = $derived.by(() =>
    cfg.placements.map((p) => (dragState && dragState.id === p.id ? { ...p, x: dragState.x, y: dragState.y } : p)),
  );
  const activePlacements = $derived(placements.filter((p) => !p.excluded));
  const excludedPlacements = $derived(placements.filter((p) => p.excluded));
  const peerConflict = $derived(!syncState.active ? (syncState.conflict ?? null) : null);
  const peerDeviceSet = $derived(new Set(peerConflict?.deviceIds ?? []));
  const conflictOverlapsIncluded = $derived(
    Boolean(peerConflict && activePlacements.some((p) => peerDeviceSet.has(p.deviceId))),
  );

  $effect(() => {
    if (!activePlacements.length) {
      selectedId = null;
      return;
    }
    if (!selectedId || !activePlacements.some((p) => p.id === selectedId)) {
      selectedId = activePlacements[0].id;
    }
  });

  const selected = $derived(activePlacements.find((p) => p.id === selectedId) ?? null);

  const includedDeviceIds = $derived(Array.from(new Set(activePlacements.map((p) => p.deviceId))));
  const includedOnlineCount = $derived(includedDeviceIds.filter((id) => deviceMap.get(id)?.online).length);

  const chosenDisplay = $derived(
    displays.find((d) => d.id === cfg.displayId) ?? displays.find((d) => d.isPrimary) ?? displays[0] ?? null,
  );
  const monitorAspect = $derived(
    preview?.aspect ?? (chosenDisplay && chosenDisplay.height > 0 ? chosenDisplay.width / chosenDisplay.height : 16 / 9),
  );
  const canvasAspect = $derived((monitorAspect * (MH / MW)).toFixed(3));

  const activeCrop = $derived<SampleRect>(preview?.crop ?? { x0: 0, y0: 0, x1: 1, y1: 1 });
  const hasLetterbox = $derived(
    syncState.active && (activeCrop.y0 > 0.025 || activeCrop.y1 < 0.975 || activeCrop.x0 > 0.025 || activeCrop.x1 < 0.975),
  );

  const selectedSample = $derived<SampleRect | null>(selected ? computeSampleRect(selected, activeCrop) : null);
  const selectedDepthPct = $derived.by(() => {
    if (!selected || !selectedSample) return 0;
    const span =
      selected.orientation === 'vertical'
        ? selectedSample.x1 - selectedSample.x0
        : selectedSample.y1 - selectedSample.y0;
    return Math.round(span * 100);
  });

  const previewByPlacement = $derived(new Map((preview?.placements ?? []).map((p) => [p.id, p.colors])));

  const monitorGlowStyle = $derived.by(() => {
    if (!syncState.active || !preview) return '';
    const [top, right, bottom, left] = preview.edgeColors;
    const rgb = (c: Rgb, a: number) => `rgba(${c.r}, ${c.g}, ${c.b}, ${a})`;
    return [
      `box-shadow:`,
      `0 -22px 48px -12px ${rgb(top, 0.42)},`,
      `26px 0 52px -12px ${rgb(right, 0.42)},`,
      `0 22px 48px -12px ${rgb(bottom, 0.42)},`,
      `-26px 0 52px -12px ${rgb(left, 0.42)}`,
    ].join(' ');
  });

  const pushConfigThrottled = throttle((next: SyncConfig) => onUpdateConfig(next), 70);

  onMount(() => {
    let alive = true;
    api
      .then(async (a) => {
        const list = await a.listDisplays().catch(() => []);
        if (alive) displays = list;
        if (alive && !document.hidden) await a.setSyncPreview(true).catch(() => {});
      })
      .catch(() => {});

    const onVis = () => {
      api.then((a) => a.setSyncPreview(!document.hidden)).catch(() => {});
    };
    document.addEventListener('visibilitychange', onVis);

    return () => {
      alive = false;
      document.removeEventListener('visibilitychange', onVis);
      api.then((a) => a.setSyncPreview(false)).catch(() => {});
    };
  });

  function pctX(x: number) {
    return (MX0 + x * MW) * 100;
  }
  function pctY(y: number) {
    return (MY0 + y * MH) * 100;
  }

  function placementName(p: SyncPlacement): string {
    const dev = deviceMap.get(p.deviceId);
    const base = dev?.name ?? p.deviceId;
    if (p.part === 'barA') return `${base} · A`;
    if (p.part === 'barB') return `${base} · B`;
    return base;
  }

  function placementIcon(p: SyncPlacement): IconName {
    return deviceMap.get(p.deviceId)?.icon ?? 'bulb';
  }

  function tokenCells(p: SyncPlacement): string[] {
    const count = p.mode === 'solid' ? 1 : Math.max(1, Math.min(16, p.segments));
    const live = previewByPlacement.get(p.id);
    if (syncState.active && live && live.length > 0) {
      return Array.from({ length: count }, (_, i) => {
        const idx = Math.min(live.length - 1, Math.floor((i * live.length) / count));
        const c = live[idx];
        return `rgb(${c.r} ${c.g} ${c.b})`;
      });
    }
    const dev = deviceMap.get(p.deviceId);
    const fallback = dev?.lastColor ?? dev?.state?.color;
    if (fallback && (fallback.r || fallback.g || fallback.b)) {
      return Array.from({ length: count }, () => `rgba(${fallback.r}, ${fallback.g}, ${fallback.b}, 0.65)`);
    }
    return Array.from({ length: count }, () => 'var(--track-strong)');
  }

  function patchPlacement(id: string, patch: Partial<SyncPlacement>, throttled = false) {
    const next: SyncConfig = {
      ...cfg,
      placements: cfg.placements.map((p) => (p.id === id ? { ...p, ...patch } : p)),
    };
    if (throttled) pushConfigThrottled(next);
    else onUpdateConfig(next);
  }

  function selectPreset(preset: SyncPreset) {
    if (preset === 'game') {
      onUpdateConfig({ ...cfg, preset: 'game', smoothing: 32, vibrance: 65, minGlow: 0 });
    } else if (preset === 'movie') {
      onUpdateConfig({ ...cfg, preset: 'movie', smoothing: 68, vibrance: 45, minGlow: 15 });
    }
  }

  function updateTuning(patch: Partial<Pick<SyncConfig, 'brightness' | 'smoothing' | 'vibrance' | 'minGlow'>>) {
    pushConfigThrottled({ ...cfg, ...patch });
  }

  function onDisplayChange(val: string) {
    onUpdateConfig({ ...cfg, displayId: val || undefined });
  }

  function triggerIdentify(id: string) {
    identifyingId = id;
    onIdentify(id);
    setTimeout(() => {
      if (identifyingId === id) identifyingId = null;
    }, 900);
  }

  function startDrag(e: PointerEvent, p: SyncPlacement) {
    if (e.button !== 0 || !canvasEl) return;
    e.preventDefault();
    selectedId = p.id;
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    dragState = { id: p.id, x: p.x, y: p.y };
  }

  function moveDrag(e: PointerEvent, p: SyncPlacement) {
    if (!dragState || dragState.id !== p.id || !canvasEl) return;
    const rect = canvasEl.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
    let nx = clamp(((e.clientX - rect.left) / rect.width - MX0) / MW, -0.4, 1.4);
    let ny = clamp(((e.clientY - rect.top) / rect.height - MY0) / MH, -0.34, 1.34);

    if (!e.shiftKey) {
      if (Math.abs(nx - 0.5) < 0.025) nx = 0.5;
      if (Math.abs(ny - 0.5) < 0.025) ny = 0.5;
    }
    nx = Math.round(nx * 1000) / 1000;
    ny = Math.round(ny * 1000) / 1000;
    dragState = { id: p.id, x: nx, y: ny };
    patchPlacement(p.id, { x: nx, y: ny }, true);
  }

  function endDrag(e: PointerEvent, p: SyncPlacement) {
    if (!dragState || dragState.id !== p.id) return;
    const final = dragState;
    dragState = null;
    try {
      (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      // Ignore if pointer capture was already released.
    }
    patchPlacement(p.id, { x: final.x, y: final.y }, false);
  }

  function nudgePlacement(e: KeyboardEvent, p: SyncPlacement) {
    let dx = 0;
    let dy = 0;
    if (e.key === 'ArrowLeft') dx = -1;
    else if (e.key === 'ArrowRight') dx = 1;
    else if (e.key === 'ArrowUp') dy = -1;
    else if (e.key === 'ArrowDown') dy = 1;
    else return;

    e.preventDefault();
    selectedId = p.id;
    const step = e.shiftKey ? 0.08 : 0.02;
    const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
    const nx = Math.round(clamp(p.x + dx * step, -0.4, 1.4) * 1000) / 1000;
    const ny = Math.round(clamp(p.y + dy * step, -0.34, 1.34) * 1000) / 1000;
    patchPlacement(p.id, { x: nx, y: ny }, false);
  }
</script>

<div class="sync-view">
  <header class="sync-header">
    <div>
      <h1>Screen sync</h1>
      <p class="sub">
        {#if syncState.active}
          Syncing {syncState.activeDevices.length} {syncState.activeDevices.length === 1 ? 'light' : 'lights'}
        {:else if peerConflict && conflictOverlapsIncluded}
          Active on {peerConflict.hostName}
        {:else}
          {includedOnlineCount} of {includedDeviceIds.length} {includedDeviceIds.length === 1 ? 'light' : 'lights'} ready
        {/if}
        {#if chosenDisplay}
          {' · '}{chosenDisplay.name} ({chosenDisplay.width}×{chosenDisplay.height})
        {/if}
      </p>
    </div>

    <div class="actions">
      {#if displays.length > 1}
        <select
          class="display-select"
          aria-label="Capture display"
          value={cfg.displayId ?? ''}
          onchange={(e) => onDisplayChange(e.currentTarget.value)}
        >
          <option value="">Primary display</option>
          {#each displays as d (d.id)}
            <option value={d.id}>
              {d.name} ({d.width}×{d.height}){d.isPrimary ? ' · Primary' : ''}
            </option>
          {/each}
        </select>
      {/if}

      <div class="segmented preset-seg" role="group" aria-label="Sync preset">
        <button class:sel={cfg.preset === 'game'} onclick={() => selectPreset('game')}>Game</button>
        <button class:sel={cfg.preset === 'movie'} onclick={() => selectPreset('movie')}>Movie</button>
      </div>

      {#if syncState.active}
        <button class="stop-btn" onclick={() => onToggleSync(false)}>
          <Icon name="stop" size={14} /> Stop sync
        </button>
      {:else if conflictOverlapsIncluded}
        <button class="primary" onclick={() => onToggleSync(true, true)} disabled={!includedOnlineCount}>
          <Icon name="play" size={14} /> Take over sync
        </button>
      {:else}
        <button class="primary" onclick={() => onToggleSync(true)} disabled={!includedOnlineCount}>
          <Icon name="play" size={14} /> Start sync
        </button>
      {/if}
    </div>
  </header>

  {#if peerConflict}
    <div class="conflict-banner" role="status">
      <div class="conflict-copy">
        <strong>
          <span class="conflict-dot"></span>
          Screen Sync is active on {peerConflict.hostName}
        </strong>
        <p>
          {#if conflictOverlapsIncluded}
            {peerConflict.deviceIds.length === 1 ? '1 light is' : `${peerConflict.deviceIds.length} lights are`} currently streaming from {peerConflict.hostName}. Light controls are paused here to avoid conflicts.
          {:else}
            {peerConflict.hostName} is syncing other lights on your network — your included lights here don't overlap.
          {/if}
        </p>
      </div>
      {#if conflictOverlapsIncluded}
        <button
          type="button"
          class="takeover-btn"
          disabled={!includedOnlineCount}
          onclick={() => onToggleSync(true, true)}
        >
          <Icon name="play" size={14} /> Take over sync
        </button>
      {/if}
    </div>
  {/if}

  <section class="stage-card">
    <div class="desk-canvas" bind:this={canvasEl} style="aspect-ratio: {canvasAspect}">
      <div class="desk-grid" aria-hidden="true"></div>

      <!-- Monitor rectangle ([0..1] × [0..1] in desk space) -->
      <div
        class="monitor"
        class:live={syncState.active}
        style="left: {MX0 * 100}%; top: {MY0 * 100}%; width: {MW * 100}%; height: {MH * 100}%; {monitorGlowStyle}"
      >
        {#if hasLetterbox}
          {#if activeCrop.y0 > 0.01}
            <div class="letterbox top" style="height: {activeCrop.y0 * 100}%"></div>
          {/if}
          {#if activeCrop.y1 < 0.99}
            <div class="letterbox bottom" style="height: {(1 - activeCrop.y1) * 100}%"></div>
          {/if}
        {/if}

        {#if selected && selectedSample}
          {@const sx = selectedSample.x0 * 100}
          {@const sy = selectedSample.y0 * 100}
          {@const sw = (selectedSample.x1 - selectedSample.x0) * 100}
          {@const sh = (selectedSample.y1 - selectedSample.y0) * 100}
          {@const slices = selected.mode === 'solid' ? 1 : Math.min(16, Math.max(1, selected.segments))}
          <div
            class="sample-zone"
            class:vert={selected.orientation === 'vertical'}
            class:horiz={selected.orientation === 'horizontal'}
            style="left: {sx}%; top: {sy}%; width: {sw}%; height: {sh}%"
          >
            {#each Array.from({ length: slices }) as _, idx (idx)}
              <span class="sample-slice"></span>
            {/each}
          </div>
        {/if}

        <div class="monitor-center">
          <Icon name="monitor" size={18} />
          <span>{chosenDisplay ? `${chosenDisplay.width}×${chosenDisplay.height}` : 'Primary display'}</span>
          {#if hasLetterbox}
            <small class="crop-pill">Letterbox crop active</small>
          {:else}
            <small>Drag lights closer for crisp edges, or further out for room wash</small>
          {/if}
        </div>
      </div>
      <div
        class="monitor-stand"
        aria-hidden="true"
        style="left: 50%; top: {(MY0 + MH) * 100}%"
      ></div>

      <!-- Draggable light placements -->
      {#each activePlacements as p (p.id)}
        {@const dev = deviceMap.get(p.deviceId)}
        {@const cells = tokenCells(p)}
        {@const isSel = selectedId === p.id}
        {@const isOffline = dev ? !dev.online : false}
        <button
          type="button"
          class="light-token"
          class:selected={isSel}
          class:horiz={p.orientation === 'horizontal'}
          class:offline={isOffline}
          class:identifying={identifyingId === p.id}
          aria-label="{placementName(p)} placement — use arrow keys to adjust position"
          style="left: {pctX(p.x)}%; top: {pctY(p.y)}%"
          onpointerdown={(e) => startDrag(e, p)}
          onpointermove={(e) => moveDrag(e, p)}
          onpointerup={(e) => endDrag(e, p)}
          onpointercancel={(e) => endDrag(e, p)}
          onkeydown={(e) => nudgePlacement(e, p)}
          onclick={() => (selectedId = p.id)}
        >
          <span class="led-strip" class:horiz={p.orientation === 'horizontal'}>
            {#each cells as bg, ci (ci)}
              <i style="background: {bg}"></i>
            {/each}
          </span>
          <span class="token-label">{placementName(p)}</span>
        </button>
      {/each}

      {#if !activePlacements.length}
        <div class="stage-empty">
          {#if devices.length}
            All lights are excluded below. Click a light chip below to place it on the stage.
          {:else}
            No lights discovered yet. Add or scan for Govee LAN lights in <b>All lights</b>.
          {/if}
        </div>
      {/if}
    </div>

    {#if selected}
      {@const selDev = deviceMap.get(selected.deviceId)}
      {@const selLockedByPeer = peerDeviceSet.has(selected.deviceId)}
      <div class="inspector">
        <div class="insp-title">
          <Icon name={placementIcon(selected)} size={16} />
          <strong>{placementName(selected)}</strong>
          {#if selDev}
            <span class="insp-meta">{selDev.sku} · {selectedDepthPct}% sample depth</span>
          {/if}
        </div>

        <div class="insp-controls">
          <button
            type="button"
            class="sm-ctl"
            title={selLockedByPeer
              ? `Synced by ${peerConflict?.hostName ?? 'another host'}`
              : 'Sweep light to identify physical position & direction'}
            disabled={(selDev ? !selDev.online : true) || selLockedByPeer}
            onclick={() => triggerIdentify(selected.id)}
          >
            <Icon name="locate" size={13} /> Identify
          </button>

          <div class="segmented mini-seg" role="group" aria-label="Orientation">
            <button
              type="button"
              class:sel={selected.orientation === 'vertical'}
              onclick={() => patchPlacement(selected.id, { orientation: 'vertical' as Orientation })}
            >Vertical</button>
            <button
              type="button"
              class:sel={selected.orientation === 'horizontal'}
              onclick={() => patchPlacement(selected.id, { orientation: 'horizontal' as Orientation })}
            >Horizontal</button>
          </div>

          {#if selected.mode === 'segments' && selected.segments > 1}
            <button
              type="button"
              class="sm-ctl"
              class:active-ctl={selected.reverse}
              title="Reverse top/bottom or left/right segment order"
              onclick={() => patchPlacement(selected.id, { reverse: !selected.reverse })}
            >
              <Icon name="flip" size={13} /> Flip
            </button>
          {/if}

          <div class="segmented mini-seg" role="group" aria-label="Color mode">
            <button
              type="button"
              class:sel={selected.mode === 'segments'}
              onclick={() =>
                patchPlacement(selected.id, {
                  mode: 'segments' as PlacementMode,
                  segments: selected.segments <= 1 ? 6 : selected.segments,
                })}
            >Zones</button>
            <button
              type="button"
              class:sel={selected.mode === 'solid'}
              onclick={() => patchPlacement(selected.id, { mode: 'solid' as PlacementMode })}
            >Solid</button>
          </div>

          {#if selected.mode === 'segments'}
            <div class="stepper" aria-label="Segment count">
              <button
                type="button"
                aria-label="Fewer zones"
                disabled={selected.segments <= 1}
                onclick={() => patchPlacement(selected.id, { segments: Math.max(1, selected.segments - 1) })}
              >−</button>
              <span>{selected.segments}</span>
              <button
                type="button"
                aria-label="More zones"
                disabled={selected.segments >= 32}
                onclick={() => patchPlacement(selected.id, { segments: Math.min(32, selected.segments + 1) })}
              >+</button>
            </div>
          {/if}

          <button
            type="button"
            class="ghost sm-ctl"
            title="Exclude from Screen Sync"
            onclick={() => patchPlacement(selected.id, { excluded: true })}
          >
            Exclude
          </button>
        </div>
      </div>
    {/if}

    {#if excludedPlacements.length > 0}
      <div class="excluded-bar">
        <span class="excluded-label">Excluded</span>
        <div class="excluded-chips">
          {#each excludedPlacements as ex (ex.id)}
            <button
              type="button"
              class="chip"
              title="Include in Screen Sync"
              onclick={() => {
                patchPlacement(ex.id, { excluded: false });
                selectedId = ex.id;
              }}
            >
              <Icon name="plus" size={12} /> {placementName(ex)}
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </section>

  <section class="tuning-grid">
    <div class="tune-card">
      <div class="tune-top">
        <span>Brightness</span>
        <strong>{cfg.brightness ?? 100}%</strong>
      </div>
      <div class="tune-slider">
        <Slider
          label="Sync brightness"
          min={1}
          max={100}
          value={cfg.brightness ?? 100}
          oninput={(v) => updateTuning({ brightness: v })}
        />
      </div>
      <p>Master ceiling for synced lights; pre-sync levels restore on stop.</p>
    </div>

    <div class="tune-card">
      <div class="tune-top">
        <span>Smoothing</span>
        <strong>{cfg.smoothing}%</strong>
      </div>
      <div class="tune-slider">
        <Slider
          label="Smoothing"
          min={0}
          max={100}
          value={cfg.smoothing}
          oninput={(v) => updateTuning({ smoothing: v })}
        />
      </div>
      <p>Adapts to fast cuts while gliding steady scenes.</p>
    </div>

    <div class="tune-card">
      <div class="tune-top">
        <span>Vibrance</span>
        <strong>{cfg.vibrance}%</strong>
      </div>
      <div class="tune-slider">
        <Slider
          label="Vibrance"
          min={0}
          max={100}
          value={cfg.vibrance}
          oninput={(v) => updateTuning({ vibrance: v })}
        />
      </div>
      <p>Deepens colours on LEDs without tinting neutral whites.</p>
    </div>

    <div class="tune-card">
      <div class="tune-top">
        <span>Dark-scene glow</span>
        <strong>{cfg.minGlow}%</strong>
      </div>
      <div class="tune-slider">
        <Slider
          label="Dark-scene glow"
          min={0}
          max={100}
          value={cfg.minGlow}
          oninput={(v) => updateTuning({ minGlow: v })}
        />
      </div>
      <p>{cfg.minGlow === 0 ? 'Lights go dark on black screens.' : 'Keeps a warm bias floor in dark scenes.'}</p>
    </div>
  </section>
</div>

<style>
  .sync-view {
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 980px;
  }

  .sync-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 4px;
  }
  h1 { margin: 0; font-size: 22px; font-weight: 600; letter-spacing: -0.02em; }
  .sub { margin: 3px 0 0; color: var(--text-2); font-size: 13px; }

  .conflict-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
    background: var(--surface);
    border: 1px solid #ffb02055;
    border-radius: 12px;
    padding: 12px 15px;
  }
  .conflict-copy {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 220px;
    flex: 1;
  }
  .conflict-copy strong {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13.5px;
    font-weight: 600;
  }
  .conflict-copy p {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-2);
    line-height: 1.4;
  }
  .conflict-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 8px var(--accent);
    flex: none;
  }
  .takeover-btn {
    background: #ffb02022;
    border-color: #ffb02066;
    color: var(--accent);
    font-weight: 550;
  }
  .takeover-btn:hover:not(:disabled) {
    background: #ffb02033;
  }

  .actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .display-select {
    font: inherit;
    font-size: 12.5px;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    height: 32px;
    padding: 0 10px;
    max-width: 240px;
    cursor: pointer;
  }
  .display-select:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; }
  .preset-seg { width: 132px; height: 32px; align-items: center; }
  .preset-seg button { height: 28px; }

  .stop-btn {
    background: #ffb02022;
    border-color: #ffb02066;
    color: var(--accent);
    font-weight: 550;
  }
  .stop-btn:hover:not(:disabled) {
    background: #ffb02033;
  }

  .stage-card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .desk-canvas {
    position: relative;
    width: 100%;
    max-height: 410px;
    min-height: 270px;
    margin: 0 auto;
    overflow: hidden;
    user-select: none;
    background: radial-gradient(circle at 50% 48%, var(--surface-2) 0%, var(--bg) 100%);
  }

  .desk-grid {
    position: absolute;
    inset: 0;
    background-image: radial-gradient(var(--border) 1px, transparent 1px);
    background-size: 22px 22px;
    opacity: 0.55;
    pointer-events: none;
  }

  .monitor {
    position: absolute;
    background: var(--surface);
    border: 2px solid var(--track-strong);
    border-radius: 10px;
    overflow: hidden;
    display: grid;
    place-items: center;
    transition: box-shadow 0.16s linear, border-color 0.2s;
  }
  .monitor.live {
    border-color: #ffffff38;
  }

  .monitor-stand {
    position: absolute;
    width: 54px;
    height: 8px;
    margin-top: 2px;
    transform: translateX(-50%);
    border-radius: 0 0 6px 6px;
    background: var(--track-strong);
    opacity: 0.7;
    pointer-events: none;
  }

  .letterbox {
    position: absolute;
    left: 0;
    right: 0;
    background: #000000a8;
    pointer-events: none;
    z-index: 1;
  }
  .letterbox.top { top: 0; border-bottom: 1px dashed #ffffff24; }
  .letterbox.bottom { bottom: 0; border-top: 1px dashed #ffffff24; }

  .sample-zone {
    position: absolute;
    background: #ffb0201f;
    border: 1.5px solid #ffb020b8;
    border-radius: 6px;
    display: flex;
    overflow: hidden;
    pointer-events: none;
    z-index: 2;
    transition: left 0.04s linear, top 0.04s linear, width 0.04s linear, height 0.04s linear;
  }
  .sample-zone.vert { flex-direction: column; }
  .sample-zone.horiz { flex-direction: row; }
  .sample-slice {
    flex: 1;
    border-bottom: 1px solid #ffb02029;
  }
  .sample-zone.horiz .sample-slice {
    border-bottom: 0;
    border-right: 1px solid #ffb02029;
  }
  .sample-slice:last-child { border: 0; }

  .monitor-center {
    position: relative;
    z-index: 3;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 12px 18px;
    text-align: center;
    color: var(--text-2);
    pointer-events: none;
  }
  .monitor-center span {
    font-size: 12.5px;
    font-weight: 550;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .monitor-center small {
    font-size: 11.5px;
    color: var(--text-3);
    max-width: 240px;
  }
  .crop-pill {
    padding: 2px 8px;
    border-radius: 99px;
    background: #ffb02024;
    color: var(--accent) !important;
  }

  .light-token {
    position: absolute;
    transform: translate(-50%, -50%);
    z-index: 5;
    height: auto;
    padding: 6px 8px;
    border-radius: 10px;
    background: var(--surface-2);
    border: 1.5px solid var(--border);
    box-shadow: 0 4px 14px #0006;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 5px;
    cursor: grab;
    touch-action: none;
    transition: border-color 0.12s, box-shadow 0.12s;
  }
  .light-token:active { cursor: grabbing; }
  .light-token:hover { border-color: var(--text-3); }
  .light-token.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px #ffb02033, 0 6px 18px #0008;
    z-index: 6;
  }
  .light-token.offline { opacity: 0.55; }
  .light-token.identifying {
    border-color: var(--accent);
    box-shadow: 0 0 0 4px #ffb02055;
  }

  .led-strip {
    width: 12px;
    height: 54px;
    border-radius: 5px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    gap: 1px;
    background: var(--bg);
    padding: 1.5px;
    box-shadow: inset 0 0 0 1px #ffffff14;
  }
  .led-strip.horiz {
    width: 64px;
    height: 12px;
    flex-direction: row;
  }
  .led-strip i {
    flex: 1;
    display: block;
    border-radius: 2px;
    transition: background 0.09s linear;
  }

  .token-label {
    font-size: 11.5px;
    font-weight: 550;
    white-space: nowrap;
    max-width: 116px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .stage-empty {
    position: absolute;
    inset: auto 20px 16px;
    text-align: center;
    font-size: 12.5px;
    color: var(--text-3);
    pointer-events: none;
  }

  .inspector {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    padding: 10px 14px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .insp-title {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .insp-title strong { font-size: 13.5px; font-weight: 550; }
  .insp-meta { font-size: 12px; color: var(--text-3); font-variant-numeric: tabular-nums; }

  .insp-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .sm-ctl {
    height: 28px;
    padding: 0 9px;
    font-size: 12px;
    border-radius: 7px;
  }
  .active-ctl {
    border-color: var(--accent);
    color: var(--accent);
  }
  .mini-seg {
    height: 28px;
    align-items: center;
  }
  .mini-seg button {
    height: 24px;
    padding: 0 9px;
    font-size: 12px;
  }

  .stepper {
    display: inline-flex;
    align-items: center;
    height: 28px;
    background: var(--track);
    border-radius: 7px;
    padding: 2px;
    gap: 2px;
  }
  .stepper button {
    width: 24px;
    height: 24px;
    padding: 0;
    border: 0;
    border-radius: 5px;
    justify-content: center;
    background: var(--surface-2);
    font-size: 13px;
  }
  .stepper span {
    min-width: 26px;
    text-align: center;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }

  .excluded-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 9px 14px;
    border-top: 1px solid var(--border);
    background: var(--bg);
  }
  .excluded-label {
    font-size: 12px;
    color: var(--text-3);
    font-weight: 500;
  }
  .excluded-chips {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .tuning-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(195px, 1fr));
    gap: 12px;
  }
  .tune-card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 13px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .tune-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 13px;
  }
  .tune-top span { font-weight: 550; }
  .tune-top strong {
    font-weight: 550;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .tune-slider {
    --fill: var(--track-strong);
  }
  .tune-card p {
    margin: 0;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.35;
  }
</style>
