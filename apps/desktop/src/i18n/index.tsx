// i18n foundation: LocaleProvider and useI18n for components, getLocale elsewhere.
//
// zh.ts defines the canonical key structure and en.ts uses `satisfies Locale`
// for completeness. The language preference lives in settings. First run
// detects and persists the system language; later saves apply immediately.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";
import { api } from "../api";
import { zh } from "./zh";
import { en } from "./en";

/** Text-table type defined by the Chinese source structure. */
export type Locale = typeof zh;
/** Supported languages. */
export type Lang = "zh" | "en";

const LOCALES: Record<Lang, Locale> = { zh, en };

// Module-level mirror for reducers and other non-component consumers.
let current: { lang: Lang; t: Locale } = { lang: "zh", t: zh };

/** Returns current text outside components; components should use useI18n. */
export function getLocale(): Locale {
  return current.t;
}

/** Defaults Chinese system locales to zh and all others to en.
 * A query parameter can override browser-only visual previews. */
export function detectSystemLang(): Lang {
  const forced = new URLSearchParams(window.location.search).get("lang");
  if (forced === "zh" || forced === "en") return forced;
  return navigator.language?.toLowerCase().startsWith("zh") ? "zh" : "en";
}

interface I18nValue {
  lang: Lang;
  t: Locale;
  setLang: (lang: Lang) => void;
}

const I18nContext = createContext<I18nValue>({
  lang: current.lang,
  t: current.t,
  setLang: () => {},
});

/** Loads the language from settings and persists system detection when empty. */
export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [lang, setLangState] = useState<Lang>(current.lang);

  const setLang = useCallback((next: Lang) => {
    current = { lang: next, t: LOCALES[next] };
    setLangState(next);
  }, []);

  useEffect(() => {
    // Browser-only previews cannot read Tauri settings.
    if (!("__TAURI_INTERNALS__" in window)) {
      setLang(detectSystemLang());
      return;
    }
    api
      .getSettings()
      .then((s) => {
        const saved = s.language === "zh" || s.language === "en" ? s.language : null;
        const lang = saved ?? detectSystemLang();
        setLang(lang);
        // Persist first-run detection for Rust shell text and future launches.
        if (!saved) {
          api.saveSettings({ ...s, language: lang }).catch(console.error);
        }
      })
      .catch(console.error);
  }, [setLang]);

  const value = useMemo(() => ({ lang, t: LOCALES[lang], setLang }), [lang, setLang]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/** Returns the current language and text table. */
export function useI18n(): I18nValue {
  return useContext(I18nContext);
}

/** Resolves an error code, returning null so callers can use the raw fallback. */
function errorText(code: string, detail?: string | null): string | null {
  const table = getLocale().errors as Record<string, string | undefined>;
  const msg = table[code];
  if (!msg) return null;
  return detail ? `${msg} (${detail})` : msg;
}

/** Formats a structured backend error or arbitrary exception for display. */
export function formatError(e: unknown): string {
  if (e && typeof e === "object" && "code" in e) {
    const { code, detail } = e as { code: string; detail?: string };
    const msg = errorText(code, detail);
    if (msg) return msg;
  }
  return String(e);
}

/** Formats an engine error code and detail, using fallback when unknown. */
export function formatErrorCode(
  code: string | null | undefined,
  detail: string | null | undefined,
  fallback: string,
): string {
  return (code ? errorText(code, detail) : null) ?? fallback;
}
