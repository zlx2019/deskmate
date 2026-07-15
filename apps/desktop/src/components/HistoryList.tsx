// 互传记录列表: 右栏"互传记录"tab 的内容(挂载时拉取, 切换 tab 即刷新)

import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { useI18n } from "../i18n";
import { humanBytes, type HistoryEntry } from "../types";
import { CardClose, ClearButton } from "./ClearButton";
import { STATUS_COLOR } from "./TransferPanel";

/** 时间短格式: 当天只显示时分, 跨天带月/日 */
function shortTime(ts: number): string {
  const d = new Date(ts);
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  return d.toDateString() === new Date().toDateString()
    ? hm
    : `${d.getMonth() + 1}/${d.getDate()} ${hm}`;
}

/** 互传记录列表(窄栏紧凑两行布局, 支持单条删除与一键清空) */
export function HistoryList() {
  const { t } = useI18n();
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);

  useEffect(() => {
    api.getHistory().then(setEntries).catch(() => setEntries([]));
  }, []);

  /** 删除单条: 先本地移除保证即时反馈, 后端删除失败仅记录 */
  const removeOne = (transferId: string) => {
    setEntries((prev) => prev?.filter((e) => e.transferId !== transferId) ?? prev);
    api.deleteHistory(transferId).catch(console.error);
  };

  /** 清空全部(ClearButton 已做二段确认) */
  const clearAll = () => {
    setEntries([]);
    api.clearHistory().catch(console.error);
  };

  if (entries === null) {
    return <div className="px-4 py-8 text-center text-xs text-mist/70">{t.history.loading}</div>;
  }
  if (entries.length === 0) {
    return <div className="px-4 py-8 text-center text-xs text-mist/70">{t.history.empty}</div>;
  }
  return (
    <>
      <div className="flex items-center justify-between px-1">
        <span className="text-[11px] text-mist">{t.history.total(entries.length)}</span>
        <ClearButton title={t.history.clear} onConfirm={clearAll} />
      </div>
      {entries.map((e) => (
        <div
          key={`${e.transferId}-${e.at}`}
          className="group relative rounded-xl border border-line bg-panel-2 px-3 py-2.5 transition-colors duration-300"
        >
          <CardClose onClick={() => removeOne(e.transferId)} />
          <div className="flex items-center gap-2">
            <span
              className={`font-gauge text-xs ${e.direction === "send" ? "text-ember" : "text-sonar"}`}
            >
              {e.direction === "send" ? "▲" : "▼"}
            </span>
            <span className="min-w-0 flex-1 truncate text-sm">
              {e.direction === "send" ? t.transfer.sendTo : t.transfer.recvFrom}
              <span className="text-fog">{e.peerName}</span>
            </span>
            <span className={`gauge-label ${STATUS_COLOR[e.status]}`}>
              {t.transfer.status[e.status]}
            </span>
          </div>
          <div className="mt-1.5 flex items-center gap-3">
            <span className="font-gauge text-[11px] text-mist">
              {t.transfer.files(e.filesDone)} · {humanBytes(e.bytes)} · {shortTime(e.at)}
            </span>
            <span className="flex-1" />
            {e.lastPath && (
              <button
                onClick={() => revealItemInDir(e.lastPath ?? "")}
                className="shrink-0 cursor-pointer rounded border border-line-2 px-2 py-0.5 text-[11px] whitespace-nowrap text-fog/80 transition-colors hover:border-sonar/50 hover:text-sonar"
              >
                {t.transfer.reveal}
              </button>
            )}
          </div>
        </div>
      ))}
    </>
  );
}
