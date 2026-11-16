// Backend abstraction. The desktop app talks to Rust via Tauri; in a plain
// browser (`npm run dev`) a mock with fake lights is used instead.
// Shapes mirror `groovie_core::model` & `groovie_core::sync` (serde JSON).

export type Rgb = { r: number; g: number; b: number };
export type DeviceState = { on: boolean; brightness: number; color: Rgb; kelvin: number };
export type DeviceIcon = 'lamp' | 'bars' | 'strip' | 'bulb';
export type KelvinRange = { min: number; max: number };
export type Device = {
  id: string;
  ip: string;
  sku: string;
  name: string;
  online: boolean;
  state: DeviceState | null;
  icon?: DeviceIcon;
  kelvinRange?: KelvinRange;
  firmware?: string;
  lastColor?: Rgb;
  lastKelvin?: number;
};
export type LogEntry = {
  tsMs: number;
  dir: string;
  addr: string;
  device?: string;
  message: string;
  poll: boolean;
};
export type ConfigInfo = {
  path?: string;
  symlinkTarget?: string;
  defaultPath?: string;
  isCustom: boolean;
  json: string;
};
export type DesktopPrefs = {
  launchAtLogin: boolean;
  startMinimized: boolean;
};
export type Group = { id: string; name: string; members: string[] };
export type SceneEntry = { device: string; on: boolean; brightness?: number; color?: Rgb; kelvin?: number };
export type Scene = { id: string; name: string; entries: SceneEntry[] };

export type DisplayInfo = {
  id: string;
  name: string;
  width: number;
  height: number;
  isPrimary: boolean;
};
export type SyncPreset = 'game' | 'movie' | 'custom';
export type PlacementPart = 'single' | 'barA' | 'barB';
export type Orientation = 'vertical' | 'horizontal';
export type PlacementMode = 'segments' | 'solid';
export type SyncPlacement = {
  id: string;
  deviceId: string;
  part: PlacementPart;
  excluded: boolean;
  x: number;
  y: number;
  orientation: Orientation;
  reverse: boolean;
  mode: PlacementMode;
  segments: number;
};
export type SyncConfig = {
  displayId?: string;
  preset: SyncPreset;
  brightness: number;
  smoothing: number;
  vibrance: number;
  minGlow: number;
  placements: SyncPlacement[];
};
export type SyncPeerConflict = {
  hostName: string;
  deviceIds: string[];
};
export type SampleRect = { x0: number; y0: number; x1: number; y1: number };
export type SyncStatus = {
  active: boolean;
  activeDevices: string[];
  config: SyncConfig;
  conflict?: SyncPeerConflict;
};
export type SyncState = {
  supported: boolean;
  active: boolean;
  activeDevices: string[];
  config: SyncConfig;
  conflict?: SyncPeerConflict;
};
export type PlacementPreview = {
  id: string;
  sample: SampleRect;
  colors: Rgb[];
};
export type SyncPreview = {
  aspect: number;
  crop: SampleRect;
  edgeColors: [Rgb, Rgb, Rgb, Rgb];
  placements: PlacementPreview[];
};

export type Action =
  | { type: 'power'; value: boolean }
  | { type: 'toggle' }
  | { type: 'brightness'; value: number }
  | { type: 'color'; value: Rgb }
  | { type: 'kelvin'; value: number };
export type Target = { type: 'device'; id: string } | { type: 'group'; id: string } | { type: 'all' };
export type GroovieEvent =
  | { type: 'deviceUpdated'; device: Device }
  | { type: 'configChanged' }
  | { type: 'logged'; entry: LogEntry }
  | { type: 'syncChanged'; status: SyncStatus }
  | { type: 'syncPreview'; preview: SyncPreview };

export interface Api {
  listDevices(): Promise<Device[]>;
  discover(): Promise<void>;
  apply(target: Target, action: Action): Promise<void>;
  rename(id: string, name: string): Promise<void>;
  removeDevice(id: string): Promise<void>;
  addIp(ip: string): Promise<void>;
  listGroups(): Promise<Group[]>;
  createGroup(name: string, members: string[]): Promise<Group>;
  updateGroup(group: Group): Promise<void>;
  deleteGroup(id: string): Promise<void>;
  listScenes(): Promise<Scene[]>;
  saveScene(name: string, devices?: string[]): Promise<Scene>;
  renameScene(id: string, name: string): Promise<Scene>;
  updateScene(id: string, name: string | null, devices?: string[]): Promise<Scene>;
  restoreScene(scene: Scene): Promise<Scene>;
  applyScene(id: string): Promise<void>;
  deleteScene(id: string): Promise<void>;
  getSyncState(): Promise<SyncState>;
  listDisplays(): Promise<DisplayInfo[]>;
  setSyncEnabled(enabled: boolean, force?: boolean): Promise<SyncState>;
  updateSyncConfig(config: SyncConfig): Promise<SyncState>;
  identifySyncPlacement(placementId: string): Promise<void>;
  setSyncPreview(enabled: boolean): Promise<void>;
  listLogs(): Promise<LogEntry[]>;
  clearLogs(): Promise<void>;
  setWindowActive(active: boolean, syncNow: boolean): Promise<void>;
  getDesktopPrefs(): Promise<DesktopPrefs>;
  setDesktopPrefs(prefs: DesktopPrefs): Promise<DesktopPrefs>;
  getConfigInfo(): Promise<ConfigInfo>;
  reloadConfig(): Promise<ConfigInfo>;
  setConfigPath(path: string | null): Promise<ConfigInfo>;
  openConfigFolder(): Promise<void>;
  openGithub(): Promise<void>;
  onEvent(cb: (e: GroovieEvent) => void): () => void;
  appVersion(): Promise<string>;
  /** Resolves to an available update, or null if up to date. */
  checkUpdate(): Promise<Update | null>;
}

export type UpdateProgress =
  | { stage: 'downloading'; percent: number | null }
  | { stage: 'installing' }
  | { stage: 'restarting' };

export type Update = {
  version: string;
  notes?: string;
  install: (onProgress?: (p: UpdateProgress) => void) => Promise<void>;
};

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
    removeDevice: (id) => invoke('remove_device', { id }),
    addIp: (ip) => invoke('add_ip', { ip }),
    listGroups: () => invoke('list_groups'),
    createGroup: (name, members) => invoke('create_group', { name, members }),
    updateGroup: (group) => invoke('update_group', { group }),
    deleteGroup: (id) => invoke('delete_group', { id }),
    listScenes: () => invoke('list_scenes'),
    saveScene: (name, devices) => invoke('save_scene', { name, devices: devices ?? null }),
    renameScene: (id, name) => invoke('rename_scene', { id, name }),
    updateScene: (id, name, devices) => invoke('update_scene', { id, name, devices: devices ?? null }),
    restoreScene: (scene) => invoke('restore_scene', { scene }),
    applyScene: (id) => invoke('apply_scene', { id }),
    deleteScene: (id) => invoke('delete_scene', { id }),
    getSyncState: () => invoke('get_sync_state'),
    listDisplays: () => invoke('list_displays'),
    setSyncEnabled: (enabled, force = false) => invoke('set_sync_enabled', { enabled, force }),
    updateSyncConfig: (config) => invoke('update_sync_config', { config }),
    identifySyncPlacement: (placementId) => invoke('identify_sync_placement', { placementId }),
    setSyncPreview: (enabled) => invoke('set_sync_preview', { enabled }),
    listLogs: () => invoke('list_logs'),
    clearLogs: () => invoke('clear_logs'),
    setWindowActive: (active, syncNow) => invoke('set_window_active', { active, syncNow }),
    getDesktopPrefs: () => invoke('get_desktop_prefs'),
    setDesktopPrefs: (prefs) => invoke('set_desktop_prefs', { prefs }),
    getConfigInfo: () => invoke('get_config_info'),
    reloadConfig: () => invoke('reload_config'),
    setConfigPath: (path) => invoke('set_config_path', { path }),
    openConfigFolder: () => invoke('open_config_folder'),
    openGithub: () => invoke('open_github'),
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
        notes: u.body || undefined,
        async install(onProgress) {
          let total: number | undefined;
          let received = 0;
          onProgress?.({ stage: 'downloading', percent: null });
          await u.download((ev) => {
            if (ev.event === 'Started') {
              total = ev.data.contentLength;
              onProgress?.({ stage: 'downloading', percent: total ? 0 : null });
            } else if (ev.event === 'Progress') {
              received += ev.data.chunkLength;
              const percent = total ? Math.min(100, Math.round((received / total) * 100)) : null;
              onProgress?.({ stage: 'downloading', percent });
            } else if (ev.event === 'Finished') {
              onProgress?.({ stage: 'downloading', percent: 100 });
            }
          });
          onProgress?.({ stage: 'installing' });
          await invoke('prepare_update_install', { beforeInstall: true });
          try {
            await u.install();
          } catch (e) {
            await invoke('discover').catch(() => {});
            throw e;
          }
          onProgress?.({ stage: 'restarting' });
          await invoke('prepare_update_install', { beforeInstall: false });
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
  const mk = (
    id: string,
    ip: string,
    sku: string,
    name: string,
    icon: DeviceIcon,
    kelvinRange: KelvinRange,
    on: boolean,
    color: Rgb,
  ): Device => ({
    id,
    ip,
    sku,
    name,
    icon,
    kelvinRange,
    online: true,
    state: { on, brightness: 80, color, kelvin: 0 },
    firmware: 'wifi 1.02.03 · ble 1.03.01',
    lastColor: color,
  });
  const devices: Device[] = [
    mk('1F:80:C5:32:32:36:72:4E', '10.0.0.182', 'H6072', 'Floor Lamp', 'lamp', { min: 2200, max: 6500 }, true, { r: 255, g: 120, b: 30 }),
    mk('3A:14:D0:88:19:40:11:2B', '10.0.0.64', 'H6056', 'Light Bars', 'bars', { min: 2000, max: 9000 }, true, { r: 120, g: 60, b: 255 }),
    {
      ...mk('AA:03:00:00:00:00:00:03', '10.0.0.99', 'H6008', 'Bulb', 'bulb', { min: 2700, max: 6500 }, false, white),
      online: false,
      state: null,
      firmware: undefined,
      lastColor: undefined,
    },
  ];
  const mockDisplays: DisplayInfo[] = [
    { id: '\\\\.\\DISPLAY1', name: 'Display 1 · LG ULTRAGEAR', width: 2560, height: 1440, isPrimary: true },
    { id: '\\\\.\\DISPLAY2', name: 'Display 2 · DELL U2723QE', width: 3840, height: 2160, isPrimary: false },
  ];
  let syncConfig: SyncConfig = {
    preset: 'game',
    brightness: 100,
    smoothing: 32,
    vibrance: 65,
    minGlow: 0,
    placements: [
      { id: '3A:14:D0:88:19:40:11:2B#A', deviceId: '3A:14:D0:88:19:40:11:2B', part: 'barA', excluded: false, x: -0.08, y: 0.5, orientation: 'vertical', reverse: false, mode: 'segments', segments: 6 },
      { id: '3A:14:D0:88:19:40:11:2B#B', deviceId: '3A:14:D0:88:19:40:11:2B', part: 'barB', excluded: false, x: 1.08, y: 0.5, orientation: 'vertical', reverse: false, mode: 'segments', segments: 6 },
      { id: '1F:80:C5:32:32:36:72:4E', deviceId: '1F:80:C5:32:32:36:72:4E', part: 'single', excluded: false, x: 1.30, y: 0.46, orientation: 'vertical', reverse: false, mode: 'segments', segments: 15 },
      { id: 'AA:03:00:00:00:00:00:03', deviceId: 'AA:03:00:00:00:00:03', part: 'single', excluded: true, x: 0.5, y: -0.14, orientation: 'vertical', reverse: false, mode: 'solid', segments: 1 },
    ],
  };
  let syncActive = false;
  let syncActiveDevices: string[] = [];
  const snapshots = new Map<string, DeviceState>();
  let previewOn = false;
  let previewTimer: ReturnType<typeof setInterval> | undefined;

  let groups: Group[] = [{ id: 'g1', name: 'Desk', members: ['3A:14:D0:88:19:40:11:2B'] }];
  let scenes: Scene[] = [];
  let desktopPrefs: DesktopPrefs = { launchAtLogin: false, startMinimized: false };
  const subs = new Set<(e: GroovieEvent) => void>();
  const emit = (e: GroovieEvent) => subs.forEach((cb) => cb(structuredClone(e)));
  const config = () => emit({ type: 'configChanged' });
  let logs: LogEntry[] = [
    { tsMs: Date.now() - 4200, dir: 'tx', addr: '239.255.255.250:4001', message: '{"msg":{"cmd":"scan","data":{"account_topic":"reserve"}}}', poll: false },
    { tsMs: Date.now() - 4120, dir: 'rx', addr: '10.0.0.182:4002', device: 'Floor Lamp (H6072)', message: '{"msg":{"cmd":"scan","data":{"ip":"10.0.0.182","device":"1F:80:C5:32:32:36:72:4E","sku":"H6072"}}}', poll: false },
    { tsMs: Date.now() - 4110, dir: 'rx', addr: '10.0.0.64:4002', device: 'Light Bars (H6056)', message: '{"msg":{"cmd":"scan","data":{"ip":"10.0.0.64","device":"3A:14:D0:88:19:40:11:2B","sku":"H6056"}}}', poll: false },
    { tsMs: Date.now() - 4050, dir: 'state', addr: '10.0.0.182', device: 'Floor Lamp (H6072)', message: 'initial state: on · 80% · #ff781e', poll: false },
    { tsMs: Date.now() - 4045, dir: 'state', addr: '10.0.0.64', device: 'Light Bars (H6056)', message: 'initial state: on · 80% · #783cff', poll: false },
    { tsMs: Date.now() - 1000, dir: 'tx', addr: '10.0.0.182:4003', device: 'Floor Lamp (H6072)', message: '{"msg":{"cmd":"devStatus","data":{}}}', poll: true },
  ];
  const pushLog = (dir: string, addr: string, device: string | undefined, message: string, poll = false) => {
    const entry: LogEntry = { tsMs: Date.now(), dir, addr, device, message, poll };
    logs = [...logs.slice(-499), entry];
    emit({ type: 'logged', entry });
  };
  let n = 0;

  const ids = (t: Target) =>
    t.type === 'all' ? devices.map((d) => d.id)
    : t.type === 'device' ? [t.id]
    : groups.find((g) => g.id === t.id)?.members ?? [];

  const set = (id: string, f: (s: DeviceState, d: Device) => void) => {
    const d = devices.find((x) => x.id === id);
    if (!d?.state) return;
    f(d.state, d);
    emit({ type: 'deviceUpdated', device: d });
  };

  const currentSyncState = (): SyncState => ({
    supported: true,
    active: syncActive,
    activeDevices: [...syncActiveDevices],
    config: structuredClone(syncConfig),
  });

  const emitSyncChanged = () => {
    emit({
      type: 'syncChanged',
      status: { active: syncActive, activeDevices: [...syncActiveDevices], config: structuredClone(syncConfig) },
    });
  };

  const sampleMockColor = (nx: number, ny: number, t: number): Rgb => {
    const wave = Math.sin(t * 1.4 + nx * 3.1 - ny * 1.8) * 0.5 + 0.5;
    const warm = Math.cos(t * 0.9 - nx * 2.4 + ny * 2.2) * 0.5 + 0.5;
    return {
      r: Math.round(35 + 210 * warm * (1 - nx * 0.35)),
      g: Math.round(45 + 150 * wave * (0.4 + ny * 0.6)),
      b: Math.round(70 + 185 * (1 - warm * 0.5) * (0.35 + nx * 0.65)),
    };
  };

  const updatePreviewLoop = () => {
    if (previewTimer) {
      clearInterval(previewTimer);
      previewTimer = undefined;
    }
    if (!previewOn || !syncActive) return;
    const tickPreview = () => {
      const t = Date.now() / 1000;
      const disp = mockDisplays.find((d) => d.id === syncConfig.displayId) ?? mockDisplays[0];
      const crop: SampleRect = { x0: 0, y0: 0, x1: 1, y1: 1 };
      const placements: PlacementPreview[] = syncConfig.placements.map((p) => {
        const sample = computeSampleRect(p, crop);
        const count = p.mode === 'solid' ? 1 : Math.max(1, Math.min(32, p.segments));
        const colors: Rgb[] = Array.from({ length: count }, (_, i) => {
          const f = (i + 0.5) / count;
          const sx = p.orientation === 'horizontal' ? sample.x0 + (sample.x1 - sample.x0) * f : (sample.x0 + sample.x1) * 0.5;
          const sy = p.orientation === 'vertical' ? sample.y0 + (sample.y1 - sample.y0) * f : (sample.y0 + sample.y1) * 0.5;
          return sampleMockColor(sx, sy, t);
        });
        return { id: p.id, sample, colors };
      });
      emit({
        type: 'syncPreview',
        preview: {
          aspect: disp.width / disp.height,
          crop,
          edgeColors: [
            sampleMockColor(0.5, 0.08, t),
            sampleMockColor(0.92, 0.5, t),
            sampleMockColor(0.5, 0.92, t),
            sampleMockColor(0.08, 0.5, t),
          ],
          placements,
        },
      });
    };
    tickPreview();
    previewTimer = setInterval(tickPreview, 100);
  };

  return {
    listDevices: async () => structuredClone(devices),
    discover: async () => {
      pushLog('tx', '239.255.255.250:4001', undefined, '{"msg":{"cmd":"scan","data":{"account_topic":"reserve"}}}');
    },
    async apply(t, a) {
      const targets = ids(t);
      if (a.type === 'toggle') {
        const anyOn = devices.some((d) => targets.includes(d.id) && d.state?.on);
        a = { type: 'power', value: !anyOn };
      }
      const act = a;
      if (syncActive && (act.type === 'color' || act.type === 'kelvin' || (act.type === 'power' && !act.value))) {
        syncActiveDevices = syncActiveDevices.filter((id) => !targets.includes(id));
        if (!syncActiveDevices.length) {
          syncActive = false;
          snapshots.clear();
          updatePreviewLoop();
        }
        emitSyncChanged();
      }
      pushLog('action', '', undefined, `apply ${JSON.stringify(t)} -> ${JSON.stringify(act)}`);
      for (const id of targets) {
        const d = devices.find((x) => x.id === id);
        if (d) {
          pushLog('tx', `${d.ip}:4003`, `${d.name} (${d.sku})`, JSON.stringify({ msg: act }));
        }
        set(id, (s, dev) => {
          if (act.type === 'power') s.on = act.value;
          if (act.type === 'brightness') s.brightness = act.value;
          if (act.type === 'color') { s.color = act.value; s.kelvin = 0; dev.lastColor = act.value; }
          if (act.type === 'kelvin') { s.kelvin = act.value; s.color = white; dev.lastKelvin = act.value; }
        });
      }
    },
    async rename(id, name) {
      const d = devices.find((x) => x.id === id);
      if (d) { d.name = name || d.sku; emit({ type: 'deviceUpdated', device: d }); }
    },
    async removeDevice(id) {
      const idx = devices.findIndex((x) => x.id === id);
      if (idx >= 0) devices.splice(idx, 1);
      groups = groups.map((g) => ({ ...g, members: g.members.filter((m) => m !== id) }));
      scenes = scenes.map((sc) => ({ ...sc, entries: sc.entries.filter((e) => e.device !== id) }));
      syncConfig = {
        ...syncConfig,
        placements: syncConfig.placements.filter((p) => p.deviceId !== id),
      };
      if (syncActiveDevices.includes(id)) {
        syncActiveDevices = syncActiveDevices.filter((x) => x !== id);
        if (!syncActiveDevices.length) {
          syncActive = false;
          snapshots.clear();
          updatePreviewLoop();
        }
      }
      emitSyncChanged();
      config();
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
    async saveScene(name, devIds) {
      const captureEntries = (ids?: string[]) =>
        devices
          .filter((d) => d.state && (!ids || ids.includes(d.id)))
          .map((d) => ({
            device: d.id,
            on: d.state!.on,
            brightness: d.state!.brightness,
            ...(d.state!.kelvin ? { kelvin: d.state!.kelvin } : { color: d.state!.color }),
          }));
      const s = { id: `s${++n}`, name, entries: captureEntries(devIds) };
      scenes.push(s); config();
      return structuredClone(s);
    },
    async renameScene(id, name) {
      const s = scenes.find((x) => x.id === id);
      if (!s) throw new Error('Scene not found');
      s.name = name;
      config();
      return structuredClone(s);
    },
    async updateScene(id, name, devIds) {
      const s = scenes.find((x) => x.id === id);
      if (!s) throw new Error('Scene not found');
      if (name != null && name.trim()) s.name = name.trim();
      s.entries = devices
        .filter((d) => d.state && (!devIds || devIds.includes(d.id)))
        .map((d) => ({
          device: d.id,
          on: d.state!.on,
          brightness: d.state!.brightness,
          ...(d.state!.kelvin ? { kelvin: d.state!.kelvin } : { color: d.state!.color }),
        }));
      config();
      return structuredClone(s);
    },
    async restoreScene(scene) {
      const idx = scenes.findIndex((x) => x.id === scene.id);
      if (idx >= 0) scenes[idx] = structuredClone(scene);
      else scenes.push(structuredClone(scene));
      config();
      return structuredClone(scene);
    },
    async applyScene(id) {
      if (syncActive) {
        syncActive = false;
        syncActiveDevices = [];
        snapshots.clear();
        updatePreviewLoop();
        emitSyncChanged();
      }
      const s = scenes.find((x) => x.id === id);
      for (const e of s?.entries ?? [])
        set(e.device, (st, dev) => {
          st.on = e.on;
          if (e.brightness) st.brightness = e.brightness;
          if (e.kelvin) { st.kelvin = e.kelvin; st.color = white; dev.lastKelvin = e.kelvin; }
          else if (e.color) { st.color = e.color; st.kelvin = 0; dev.lastColor = e.color; }
        });
    },
    async deleteScene(id) { scenes = scenes.filter((s) => s.id !== id); config(); },
    getSyncState: async () => currentSyncState(),
    listDisplays: async () => structuredClone(mockDisplays),
    async setSyncEnabled(enabled) {
      if (enabled) {
        const includedIds = Array.from(new Set(syncConfig.placements.filter((p) => !p.excluded).map((p) => p.deviceId)));
        const onlineIds = includedIds.filter((id) => devices.find((d) => d.id === id)?.online);
        if (!onlineIds.length) throw new Error('No online lights are included in Screen Sync');
        const bri = Math.max(1, Math.min(100, syncConfig.brightness ?? 100));
        for (const id of onlineIds) {
          const d = devices.find((x) => x.id === id);
          if (d?.state && !snapshots.has(id)) snapshots.set(id, structuredClone(d.state));
          set(id, (s) => { s.on = true; s.brightness = bri; });
        }
        syncActive = true;
        syncActiveDevices = onlineIds;
        pushLog('action', '', undefined, `Screen Sync started (${onlineIds.length} lights)`);
      } else {
        syncActive = false;
        syncActiveDevices = [];
        for (const [id, snap] of snapshots.entries()) {
          set(id, (s) => Object.assign(s, snap));
        }
        snapshots.clear();
        pushLog('action', '', undefined, 'Screen Sync stopped');
      }
      updatePreviewLoop();
      emitSyncChanged();
      return currentSyncState();
    },
    async updateSyncConfig(next) {
      const preset: SyncPreset =
        next.smoothing === 32 && next.vibrance === 65 && next.minGlow === 0
          ? 'game'
          : next.smoothing === 68 && next.vibrance === 45 && next.minGlow === 15
            ? 'movie'
            : 'custom';
      const brightness = Math.max(1, Math.min(100, Math.round(next.brightness ?? 100)));
      syncConfig = { ...structuredClone(next), brightness, preset };
      if (syncActive) {
        const includedIds = new Set(syncConfig.placements.filter((p) => !p.excluded).map((p) => p.deviceId));
        syncActiveDevices = syncActiveDevices.filter((id) => includedIds.has(id));
        for (const id of syncActiveDevices) {
          set(id, (s) => { s.brightness = brightness; });
        }
        if (!syncActiveDevices.length) {
          syncActive = false;
        }
      }
      updatePreviewLoop();
      emitSyncChanged();
      config();
      return currentSyncState();
    },
    async identifySyncPlacement(placementId) {
      pushLog('action', '', undefined, `Identify placement ${placementId}`);
    },
    async setSyncPreview(enabled) {
      previewOn = enabled;
      updatePreviewLoop();
    },
    listLogs: async () => structuredClone(logs),
    clearLogs: async () => { logs = []; },
    setWindowActive: async () => {},
    getDesktopPrefs: async () => ({ ...desktopPrefs }),
    async setDesktopPrefs(prefs) {
      desktopPrefs = { ...prefs };
      return { ...desktopPrefs };
    },
    async getConfigInfo() {
      const nicknames = Object.fromEntries(devices.map((d) => [d.id, d.name]));
      const known_devices = devices.map((d) => ({ id: d.id, ip: d.ip, sku: d.sku }));
      return {
        path: '~/.config/groovie/groovie.json',
        defaultPath: '~/.config/groovie/groovie.json',
        isCustom: false,
        json: JSON.stringify({ version: 1, nicknames, groups, scenes, known_devices, sync: syncConfig }, null, 2),
      };
    },
    async reloadConfig() {
      return this.getConfigInfo();
    },
    async setConfigPath(path) {
      const base = await this.getConfigInfo();
      return {
        ...base,
        path: path?.trim() || base.defaultPath,
        isCustom: Boolean(path?.trim()),
      };
    },
    openConfigFolder: async () => {},
    openGithub: async () => {
      window.open('https://github.com/rudyorre/groovie', '_blank', 'noopener,noreferrer');
    },
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

/** Mirror of `groovie_core::sync::compute_sample_rect` for 60 FPS drag overlays. */
export function computeSampleRect(
  p: Pick<SyncPlacement, 'x' | 'y' | 'orientation'>,
  crop: SampleRect = { x0: 0, y0: 0, x1: 1, y1: 1 },
): SampleRect {
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
  const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
  const dxOut = p.x < 0 ? -p.x : p.x > 1 ? p.x - 1 : 0;
  const dyOut = p.y < 0 ? -p.y : p.y > 1 ? p.y - 1 : 0;
  const dist = Math.hypot(dxOut, dyOut);
  const t = clamp((dist - 0.03) / 0.35, 0, 1);
  const depth = lerp(0.13, 0.44, t);

  let nx0: number, ny0: number, nx1: number, ny1: number;
  if (p.orientation === 'vertical') {
    const anchorX = clamp(p.x, 0, 1);
    nx0 = anchorX * (1 - depth);
    nx1 = nx0 + depth;
    const offCenter = Math.max(0, Math.abs(p.y - 0.5) - 0.15) / 0.5;
    const spanY = clamp(lerp(0.92, 1.0, t) - 0.32 * offCenter, 0.45, 1.0);
    const cy = clamp(p.y, spanY * 0.5, 1 - spanY * 0.5);
    ny0 = cy - spanY * 0.5;
    ny1 = cy + spanY * 0.5;
  } else {
    const anchorY = clamp(p.y, 0, 1);
    ny0 = anchorY * (1 - depth);
    ny1 = ny0 + depth;
    const offCenter = Math.max(0, Math.abs(p.x - 0.5) - 0.15) / 0.5;
    const spanX = clamp(lerp(0.92, 1.0, t) - 0.32 * offCenter, 0.45, 1.0);
    const cx = clamp(p.x, spanX * 0.5, 1 - spanX * 0.5);
    nx0 = cx - spanX * 0.5;
    nx1 = cx + spanX * 0.5;
  }

  const cw = Math.max(0.1, crop.x1 - crop.x0);
  const ch = Math.max(0.1, crop.y1 - crop.y0);
  return {
    x0: crop.x0 + nx0 * cw,
    y0: crop.y0 + ny0 * ch,
    x1: crop.x0 + nx1 * cw,
    y1: crop.y0 + ny1 * ch,
  };
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
