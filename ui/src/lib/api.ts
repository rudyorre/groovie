// Backend abstraction. The desktop app talks to Rust via Tauri; in a plain
// browser (`npm run dev`) a mock with fake lights is used instead.
// Shapes mirror `groovie_core::model` (serde JSON).

export type Rgb = { r: number; g: number; b: number };
export type DeviceState = { on: boolean; brightness: number; color: Rgb; kelvin: number };
export type Device = {
  id: string;
  ip: string;
  sku: string;
  name: string;
  online: boolean;
  state: DeviceState | null;
};
export type Group = { id: string; name: string; members: string[] };
export type SceneEntry = { device: string; on: boolean; brightness?: number; color?: Rgb; kelvin?: number };
export type Scene = { id: string; name: string; entries: SceneEntry[] };

export type Action =
  | { type: 'power'; value: boolean }
  | { type: 'toggle' }
  | { type: 'brightness'; value: number }
  | { type: 'color'; value: Rgb }
  | { type: 'kelvin'; value: number };
export type Target = { type: 'device'; id: string } | { type: 'group'; id: string } | { type: 'all' };
export type GroovieEvent = { type: 'deviceUpdated'; device: Device } | { type: 'configChanged' };

export interface Api {
  listDevices(): Promise<Device[]>;
  discover(): Promise<void>;
  apply(target: Target, action: Action): Promise<void>;
  rename(id: string, name: string): Promise<void>;
  addIp(ip: string): Promise<void>;
  listGroups(): Promise<Group[]>;
  createGroup(name: string, members: string[]): Promise<Group>;
  updateGroup(group: Group): Promise<void>;
  deleteGroup(id: string): Promise<void>;
  listScenes(): Promise<Scene[]>;
  saveScene(name: string): Promise<Scene>;
  applyScene(id: string): Promise<void>;
  deleteScene(id: string): Promise<void>;
  onEvent(cb: (e: GroovieEvent) => void): () => void;
  appVersion(): Promise<string>;
  /** Resolves to an available update, or null if up to date. */
  checkUpdate(): Promise<Update | null>;
}

export type Update = { version: string; install: () => Promise<void> };

export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

// ---- Tauri ---------------------------------------------------------------

async function tauriApi(): Promise<Api> {
  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');
  return {
    listDevices: () => invoke('list_devices'),
    discover: () => invoke('discover'),
    apply: (target, action) => invoke('apply', { target, action }),
    rename: (id, name) => invoke('rename', { id, name }),
    addIp: (ip) => invoke('add_ip', { ip }),
    listGroups: () => invoke('list_groups'),
    createGroup: (name, members) => invoke('create_group', { name, members }),
    updateGroup: (group) => invoke('update_group', { group }),
    deleteGroup: (id) => invoke('delete_group', { id }),
    listScenes: () => invoke('list_scenes'),
    saveScene: (name) => invoke('save_scene', { name }),
    applyScene: (id) => invoke('apply_scene', { id }),
    deleteScene: (id) => invoke('delete_scene', { id }),
    onEvent(cb) {
      const un = listen<GroovieEvent>('groovie://event', (e) => cb(e.payload));
      return () => void un.then((f) => f());
    },
    appVersion: async () => (await import('@tauri-apps/api/app')).getVersion(),
    async checkUpdate() {
      const { check } = await import('@tauri-apps/plugin-updater');
      const u = await check();
      if (!u) return null;
      return {
        version: u.version,
        async install() {
          await u.downloadAndInstall();
          const { relaunch } = await import('@tauri-apps/plugin-process');
          await relaunch();
        },
      };
    },
  };
}

// ---- Mock (browser dev) -------------------------------------------------

function mockApi(): Api {
  const white: Rgb = { r: 255, g: 255, b: 255 };
  const mk = (id: string, ip: string, sku: string, name: string, on: boolean, color: Rgb): Device => ({
    id, ip, sku, name, online: true, state: { on, brightness: 80, color, kelvin: 0 },
  });
  const devices: Device[] = [
    mk('AA:01', '192.168.1.21', 'H6072', 'Floor Lamp', true, { r: 255, g: 120, b: 30 }),
    mk('AA:02', '192.168.1.22', 'H6056', 'Light Bars', true, { r: 120, g: 60, b: 255 }),
    { ...mk('AA:03', '192.168.1.23', 'H6008', 'Bulb', false, white), online: false, state: null },
  ];
  let groups: Group[] = [{ id: 'g1', name: 'Desk', members: ['AA:02'] }];
  let scenes: Scene[] = [];
  const subs = new Set<(e: GroovieEvent) => void>();
  const emit = (e: GroovieEvent) => subs.forEach((cb) => cb(structuredClone(e)));
  const config = () => emit({ type: 'configChanged' });
  let n = 0;

  const ids = (t: Target) =>
    t.type === 'all' ? devices.map((d) => d.id)
    : t.type === 'device' ? [t.id]
    : groups.find((g) => g.id === t.id)?.members ?? [];

  const set = (id: string, f: (s: DeviceState) => void) => {
    const d = devices.find((x) => x.id === id);
    if (!d?.state) return;
    f(d.state);
    emit({ type: 'deviceUpdated', device: d });
  };

  return {
    listDevices: async () => structuredClone(devices),
    discover: async () => {},
    async apply(t, a) {
      const targets = ids(t);
      if (a.type === 'toggle') {
        const anyOn = devices.some((d) => targets.includes(d.id) && d.state?.on);
        a = { type: 'power', value: !anyOn };
      }
      const act = a;
      for (const id of targets)
        set(id, (s) => {
          if (act.type === 'power') s.on = act.value;
          if (act.type === 'brightness') s.brightness = act.value;
          if (act.type === 'color') { s.color = act.value; s.kelvin = 0; }
          if (act.type === 'kelvin') s.kelvin = act.value;
        });
    },
    async rename(id, name) {
      const d = devices.find((x) => x.id === id);
      if (d) { d.name = name || d.sku; emit({ type: 'deviceUpdated', device: d }); }
    },
    addIp: async () => {},
    listGroups: async () => structuredClone(groups),
    async createGroup(name, members) {
      const g = { id: `g${++n}x`, name, members };
      groups.push(g); config();
      return g;
    },
    async updateGroup(g) { groups = groups.map((x) => (x.id === g.id ? g : x)); config(); },
    async deleteGroup(id) { groups = groups.filter((g) => g.id !== id); config(); },
    listScenes: async () => structuredClone(scenes),
    async saveScene(name) {
      const entries = devices.filter((d) => d.state).map((d) => ({
        device: d.id, on: d.state!.on, brightness: d.state!.brightness,
        ...(d.state!.kelvin ? { kelvin: d.state!.kelvin } : { color: d.state!.color }),
      }));
      const s = { id: `s${++n}`, name, entries };
      scenes.push(s); config();
      return s;
    },
    async applyScene(id) {
      const s = scenes.find((x) => x.id === id);
      for (const e of s?.entries ?? [])
        set(e.device, (st) => {
          st.on = e.on;
          if (e.brightness) st.brightness = e.brightness;
          if (e.kelvin) st.kelvin = e.kelvin;
          else if (e.color) { st.color = e.color; st.kelvin = 0; }
        });
    },
    async deleteScene(id) { scenes = scenes.filter((s) => s.id !== id); config(); },
    onEvent(cb) { subs.add(cb); return () => subs.delete(cb); },
    appVersion: async () => 'demo',
    checkUpdate: async () => null,
  };
}

export const api: Promise<Api> = isTauri ? tauriApi() : Promise.resolve(mockApi());

// ---- helpers ------------------------------------------------------------

export const toHex = ({ r, g, b }: Rgb) =>
  '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('');

export const fromHex = (h: string): Rgb => {
  const v = parseInt(h.replace('#', ''), 16);
  return { r: (v >> 16) & 255, g: (v >> 8) & 255, b: v & 255 };
};

/** Approximate RGB for a colour temperature, for tinting the UI. */
export function kelvinToRgb(k: number): Rgb {
  const t = k / 100;
  const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
  const r = t <= 66 ? 255 : 329.7 * Math.pow(t - 60, -0.1332);
  const g = t <= 66 ? 99.47 * Math.log(t) - 161.12 : 288.12 * Math.pow(t - 60, -0.0755);
  const b = t >= 66 ? 255 : t <= 19 ? 0 : 138.52 * Math.log(t - 10) - 305.04;
  return { r: clamp(r), g: clamp(g), b: clamp(b) };
}

/** Leading + trailing throttle so sliders feel live without flooding UDP. */
export function throttle<A extends unknown[]>(fn: (...a: A) => void, ms = 80) {
  let last = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pending: A | undefined;
  return (...a: A) => {
    const now = Date.now();
    pending = a;
    if (now - last >= ms) {
      last = now;
      fn(...a);
      pending = undefined;
    } else if (!timer) {
      timer = setTimeout(() => {
        timer = undefined;
        last = Date.now();
        if (pending) fn(...pending);
        pending = undefined;
      }, ms - (now - last));
    }
  };
}
