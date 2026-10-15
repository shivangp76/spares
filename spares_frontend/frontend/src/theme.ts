import { useSyncExternalStore } from 'react';

export type ThemePreference = 'light' | 'dark' | 'system';

// Also read by the inline script in index.html, which applies the theme before the first paint
const STORAGE_KEY = 'spares_theme';
const CHANGE_EVENT = 'spares-theme-change';
const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');

export function getThemePreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === 'light' || stored === 'dark') return stored;
  } catch {
    // Without storage the preference can't be remembered, so the system's is used
  }
  return 'system';
}

/** Sets `data-theme` on the root element, which index.css uses to pick the colours. */
export function applyThemePreference(preference: ThemePreference): void {
  if (preference === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = preference;
}

export function setThemePreference(preference: ThemePreference): void {
  try {
    if (preference === 'system') localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, preference);
  } catch {
    // The theme still applies for this page load
  }
  applyThemePreference(preference);
  window.dispatchEvent(new Event(CHANGE_EVENT));
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener(CHANGE_EVENT, onChange);
  darkQuery.addEventListener('change', onChange);
  return () => {
    window.removeEventListener(CHANGE_EVENT, onChange);
    darkQuery.removeEventListener('change', onChange);
  };
}

export function useThemePreference(): ThemePreference {
  return useSyncExternalStore(subscribe, getThemePreference);
}

/** The theme actually shown, for components styled outside of CSS such as CodeMirror. */
export function useResolvedTheme(): 'light' | 'dark' {
  return useSyncExternalStore(subscribe, () => {
    const preference = getThemePreference();
    if (preference !== 'system') return preference;
    return darkQuery.matches ? 'dark' : 'light';
  });
}

export type TypstThemePreference = 'light' | 'dark' | 'interface';

// Typst pages are shown in the interface's theme unless the Settings page or a viewer's toggle
// picks one. Either is remembered, and is shared by every viewer.
const TYPST_STORAGE_KEY = 'spares_typst_theme';
const TYPST_CHANGE_EVENT = 'spares-typst-theme-change';

export function getTypstThemePreference(): TypstThemePreference {
  try {
    const stored = localStorage.getItem(TYPST_STORAGE_KEY);
    if (stored === 'light' || stored === 'dark') return stored;
  } catch {
    // Without storage the preference can't be remembered, so the interface's theme is used
  }
  return 'interface';
}

// Also kept in memory, so the toggle still works for this page load without storage
let typstPreference = getTypstThemePreference();

export function setTypstThemePreference(preference: TypstThemePreference): void {
  typstPreference = preference;
  try {
    if (preference === 'interface') localStorage.removeItem(TYPST_STORAGE_KEY);
    else localStorage.setItem(TYPST_STORAGE_KEY, preference);
  } catch {
    // The preference still applies for this page load
  }
  window.dispatchEvent(new Event(TYPST_CHANGE_EVENT));
}

function subscribeTypst(onChange: () => void): () => void {
  window.addEventListener(TYPST_CHANGE_EVENT, onChange);
  return () => window.removeEventListener(TYPST_CHANGE_EVENT, onChange);
}

export function useTypstThemePreference(): TypstThemePreference {
  return useSyncExternalStore(subscribeTypst, () => typstPreference);
}

/** The theme Typst pages are shown in. */
export function useTypstTheme(): 'light' | 'dark' {
  const resolved = useResolvedTheme();
  const preference = useTypstThemePreference();
  return preference === 'interface' ? resolved : preference;
}
