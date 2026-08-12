import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import pkg from "./package.json";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// The UI library bundles Noto Sans SC CJK subsets (~3.5 MB, the single largest
// installer contributor). Drop them at build time: Chinese text falls back to
// the system font (PingFang SC / Microsoft YaHei), which renders better anyway.
// Dev mode is untouched, and the small latin subsets stay bundled.
function dropCjkFonts(): Plugin {
  const CJK_FONT = /noto-sans-sc-chinese-simplified-/;
  return {
    name: "drop-cjk-fonts",
    apply: "build",
    generateBundle(_options, bundle) {
      for (const [name, asset] of Object.entries(bundle)) {
        if (CJK_FONT.test(name)) {
          delete bundle[name];
        } else if (asset.type === "asset" && name.endsWith(".css")) {
          // Also remove the @font-face blocks that reference the dropped files
          // so the webview never requests missing assets.
          asset.source = asset.source
            .toString()
            .replace(/@font-face\{[^{}]*noto-sans-sc-chinese-simplified-[^{}]*\}/g, "");
        }
      }
    },
  };
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss(), dropCjkFonts()],

  // package.json is the single version source injected for the frontend About view.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
