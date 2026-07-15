// i18n 基座: LocaleProvider + useI18n(组件用)+ getLocale(非组件模块用)
//
// 文案文件: zh.ts(权威键源)/ en.ts(satisfies Locale 保证键完备)。
// 语言偏好存在设置(settings.language); 首次启动为空时按系统语言检测并写回,
// 之后以设置为准 —— 设置页可随时切换, 保存后即时生效。

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

/** 文案表类型: 以中文文件的结构为准 */
export type Locale = typeof zh;
/** 支持的语言 */
export type Lang = "zh" | "en";

const LOCALES: Record<Lang, Locale> = { zh, en };

// 模块级镜像: 供 reducer 默认值等非组件上下文取当前文案(Provider 切换时同步)
let current: { lang: Lang; t: Locale } = { lang: "zh", t: zh };

/** 当前文案表(非组件模块用; 组件内请用 useI18n 以随切换重渲染) */
export function getLocale(): Locale {
  return current.t;
}

/** 按系统语言判定默认语言: 中文系走 zh, 其余 en
 * (?lang=en 查询参数可覆盖, 供浏览器预览的视觉调试用; Tauri 内无查询参数) */
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

/** 语言上下文: 启动时从设置读取(为空则按系统语言检测并写回持久化) */
export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [lang, setLangState] = useState<Lang>(current.lang);

  const setLang = useCallback((next: Lang) => {
    current = { lang: next, t: LOCALES[next] };
    setLangState(next);
  }, []);

  useEffect(() => {
    // 纯浏览器预览(无 Tauri)下读不到设置, 仅按系统语言展示
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
        // 首启把检测结果写回设置: 托盘/通知(Rust 侧)与后续启动都以设置为准
        if (!saved) {
          api.saveSettings({ ...s, language: lang }).catch(console.error);
        }
      })
      .catch(console.error);
  }, [setLang]);

  const value = useMemo(() => ({ lang, t: LOCALES[lang], setLang }), [lang, setLang]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/** 取当前语言与文案表 */
export function useI18n(): I18nValue {
  return useContext(I18nContext);
}
