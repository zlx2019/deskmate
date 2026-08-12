// UI style and panel-color preferences stored in localStorage:
// - Style switches the full variable set through data-theme and synchronizes
//   native window chrome.
// - The light-theme panel palette derives surfaces and borders from a base
//   color. The default uses static index.css values.

import { getCurrentWindow } from "@tauri-apps/api/window";

/** Panel base-color presets, with the default first. All are low-saturation
 * pastels that sit well next to the teal accent and the grassland map. */
export const PANEL_PRESETS = [
  "#ffffff", // Pure white, deriving a neutral grayscale.
  "#ddf3d8", // Sprout green.
  "#dff2ee", // Mint teal, echoing the accent color.
  "#faf3da", // Butter cream, the classic island interior.
  "#e3edf8", // Pale sky, echoing lakes and clouds.
] as const;

/** Default panel base color matching the static index.css palette. */
export const DEFAULT_PANEL_COLOR = PANEL_PRESETS[0];

const STORAGE_KEY = "dm-panel-color";

/** Converts #rrggbb to [h, s, l] ranges of 0-360, 0-100, and 0-100. */
function hexToHsl(hex: string): [number, number, number] {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l * 100];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === r) h = ((g - b) / d + (g < b ? 6 : 0)) * 60;
  else if (max === g) h = ((b - r) / d + 2) * 60;
  else h = ((r - g) / d + 4) * 60;
  return [h, s * 100, l * 100];
}

/** Converts [h, s, l] to #rrggbb. */
function hslToHex(h: number, s: number, l: number): string {
  const sn = Math.min(100, Math.max(0, s)) / 100;
  const ln = Math.min(100, Math.max(0, l)) / 100;
  const a = sn * Math.min(ln, 1 - ln);
  const f = (n: number) => {
    const k = (n + h / 30) % 12;
    const c = ln - a * Math.max(-1, Math.min(k - 3, Math.min(9 - k, 1)));
    return Math.round(c * 255)
      .toString(16)
      .padStart(2, "0");
  };
  return `#${f(0)}${f(8)}${f(4)}`;
}

/** Derives the six panel colors with stepped lightness and muted borders. */
function derivePalette(hex: string): Record<string, string> {
  const [h, s, l] = hexToHsl(hex);
  return {
    "--color-panel": hex,
    "--color-panel-2": hslToHex(h, s, l - 5),
    "--color-line": hslToHex(h, s, l - 11),
    "--color-line-2": hslToHex(h, s, l - 22),
    "--color-edge": hslToHex(h, s * 0.55, l - 33),
    "--color-edge-2": hslToHex(h, s * 0.5, l - 52),
  };
}

/** Normalizes optional-# user input to lowercase, returning null when invalid. */
export function normalizeHex(input: string): string | null {
  const value = input.trim().toLowerCase();
  const hex = value.startsWith("#") ? value : `#${value}`;
  return /^#[0-9a-f]{6}$/.test(hex) ? hex : null;
}

/** Applies a panel color, clearing inline overrides for the default. */
export function applyPanelColor(hex: string): void {
  const root = document.documentElement.style;
  const palette = derivePalette(hex);
  for (const [name, value] of Object.entries(palette)) {
    if (hex === DEFAULT_PANEL_COLOR) root.removeProperty(name);
    else root.setProperty(name, value);
  }
}

/** Reads the stored panel color, falling back to the default. */
export function loadPanelColor(): string {
  try {
    return normalizeHex(localStorage.getItem(STORAGE_KEY) ?? "") ?? DEFAULT_PANEL_COLOR;
  } catch {
    return DEFAULT_PANEL_COLOR;
  }
}

/** Stores the panel color, removing storage for the default. */
export function savePanelColor(hex: string): void {
  try {
    if (hex === DEFAULT_PANEL_COLOR) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, hex);
  } catch {
    // Ignore unavailable localStorage; only persistence is affected.
  }
}

/* ---------- Interface style ---------- */

export type StyleMode = "light" | "dark";

const STYLE_KEY = "dm-style";

/** Applies data-theme and synchronizes panel colors and native window chrome. */
export function applyStyle(mode: StyleMode): void {
  const root = document.documentElement;
  if (mode === "dark") {
    root.dataset.theme = "dark";
    // Dark mode has a fixed palette; remove higher-priority light overrides.
    for (const name of Object.keys(derivePalette(DEFAULT_PANEL_COLOR))) {
      root.style.removeProperty(name);
    }
  } else {
    delete root.dataset.theme;
    // Restore the user's panel palette when returning to light mode.
    applyPanelColor(loadPanelColor());
  }
  // Synchronize native title-bar and border appearance when Tauri is available.
  if ("__TAURI_INTERNALS__" in window) {
    getCurrentWindow()
      .setTheme(mode === "dark" ? "dark" : "light")
      .catch(console.error);
  }
}

/** Reads the stored interface style, defaulting to light. */
export function loadStyle(): StyleMode {
  try {
    return localStorage.getItem(STYLE_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

/** Stores the interface style, removing storage for the light default. */
export function saveStyle(mode: StyleMode): void {
  try {
    if (mode === "light") localStorage.removeItem(STYLE_KEY);
    else localStorage.setItem(STYLE_KEY, mode);
  } catch {
    // Ignore unavailable localStorage; only persistence is affected.
  }
}

/** Restores style and panel color before rendering to avoid a default-style flash. */
export function initTheme(): void {
  applyStyle(loadStyle());
}
