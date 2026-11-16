export type ThemeMode = 'system' | 'dark' | 'light';

const THEME_STORAGE_KEY = 'groovie:theme';

export function readTheme(): ThemeMode {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    if (raw === 'dark' || raw === 'light' || raw === 'system') return raw;
  } catch {
    // Ignore storage access errors in restricted webviews.
  }
  return 'system';
}

export function applyTheme(mode: ThemeMode): void {
  if (typeof document !== 'undefined') {
    if (mode === 'system') {
      delete document.documentElement.dataset.theme;
    } else {
      document.documentElement.dataset.theme = mode;
    }
  }
  try {
    localStorage.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    // Ignore storage access errors in restricted webviews.
  }
}
