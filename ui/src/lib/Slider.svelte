<script lang="ts">
  // Chunky "fill" slider (brightness) or gradient slider (colour temperature).
  // Keeps its own value while dragging so remote updates don't fight the thumb.
  let {
    value,
    min = 0,
    max = 100,
    step = 1,
    variant = 'fill',
    disabled = false,
    label,
    oninput,
  }: {
    value: number;
    min?: number;
    max?: number;
    step?: number;
    variant?: 'fill' | 'kelvin';
    disabled?: boolean;
    label: string;
    oninput: (v: number) => void;
  } = $props();

  let local = $state(0);
  let dragging = $state(false);
  $effect(() => {
    if (!dragging) local = value;
  });
  const pct = $derived(((local - min) / (max - min)) * 100);
</script>

<input
  type="range"
  class={variant}
  {min}
  {max}
  {step}
  {disabled}
  aria-label={label}
  value={local}
  style="--p: {pct}%"
  onpointerdown={() => (dragging = true)}
  onpointerup={() => (dragging = false)}
  onblur={() => (dragging = false)}
  oninput={(e) => {
    local = +e.currentTarget.value;
    oninput(local);
  }}
/>

<style>
  input {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    margin: 0;
    background: transparent;
    cursor: pointer;
  }
  input:disabled { cursor: not-allowed; opacity: 0.4; }
  input:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; border-radius: 10px; }

  /* Fill variant: the whole track is the control. */
  .fill { height: 32px; }
  .fill::-webkit-slider-runnable-track {
    height: 32px;
    border-radius: 9px;
    background: linear-gradient(to right, var(--fill) var(--p), var(--track) var(--p));
  }
  .fill::-moz-range-track { height: 32px; border-radius: 9px; background: var(--track); }
  .fill::-moz-range-progress { height: 32px; border-radius: 9px 0 0 9px; background: var(--fill); }
  .fill::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 12px;
    height: 32px;
    background: transparent;
  }
  .fill::-moz-range-thumb { width: 12px; height: 32px; border: 0; background: transparent; }

  /* Kelvin variant: warm → cool gradient with a round thumb. */
  .kelvin { height: 20px; }
  .kelvin::-webkit-slider-runnable-track {
    height: 8px;
    border-radius: 4px;
    background: linear-gradient(to right, #ff9d3b, #ffd7a8, #fff4e8, #dce8ff, #b8ceff);
  }
  .kelvin::-moz-range-track {
    height: 8px;
    border-radius: 4px;
    background: linear-gradient(to right, #ff9d3b, #ffd7a8, #fff4e8, #dce8ff, #b8ceff);
  }
  .kelvin::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 18px;
    height: 18px;
    margin-top: -5px;
    border-radius: 50%;
    background: #fff;
    border: 0;
    box-shadow: 0 0 0 1px #0003, 0 1px 3px #0005;
  }
  .kelvin::-moz-range-thumb { width: 18px; height: 18px; border: 0; border-radius: 50%; background: #fff; box-shadow: 0 0 0 1px #0003, 0 1px 3px #0005; }
</style>
