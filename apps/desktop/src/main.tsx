// Must execute first for the library Notification react-dom compatibility shim.
import "./react-dom-shim";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { I18nProvider } from "./i18n";
// Component-library styles, including --animal-* variables and bundled fonts.
import "animal-island-ui/style";
import "./index.css";
import { initTheme } from "./theme";

// Restore style and panel color before rendering to avoid a default-style flash.
initTheme();

// Suppress the WebView's default context menu (Reload etc.) in production
// builds; editable fields keep the native copy/paste menu.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (e) => {
    const el = e.target instanceof Element ? e.target : null;
    if (!el?.closest("input, textarea, [contenteditable='true']")) e.preventDefault();
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </React.StrictMode>,
);
