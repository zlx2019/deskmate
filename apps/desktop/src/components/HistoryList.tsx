// 互传记录列表: 右栏"互传记录"tab 的内容(挂载时拉取, 切换 tab 即刷新)

import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { humanBytes, type HistoryEntry } from "../types";

/** 状态文案与颜色(与传输卡片的语义一致) */
const STATUS_META: Record<HistoryEntry["status"], { label: string; color: string }> = {
  completed: { label: "已完成", color: "text-sonar" },
  cancelled: { label: "已取消", color: "text-mist" },
  interrupted: { label: "已中断", color: "text-alert" },
  rejected: { label: "被拒绝", color: "text-alert" },
};

/** 时间短格式: 当天只显示时分, 跨天带月/日 */
function shortTime(ts: number): string {
  const d = new Date(ts);
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  return d.toDateString() === new Date().toDateString()
    ? hm
    : `${d.getMonth() + 1}/${d.getDate()} ${hm}`;
}

/** 互传记录列表(窄栏紧凑两行布局) */
export function HistoryList() {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);

  useEffect(() => {
    api.getHistory().then(setEntries).catch(() => setEntries([]));
  }, []);

  if (entries === null) {
    return <div className="px-4 py-8 text-center text-xs text-mist/70">加载中…</div>;
  }
  if (entries.length === 0) {
    return <div className="px-4 py-8 text-center text-xs text-mist/70">暂无互传记录</div>;
  }
  return (
    <>
      {entries.map((e) => (
        <div
          key={`${e.transferId}-${e.at}`}
          className="rounded-xl border border-line bg-panel-2 px-3 py-2.5 transition-colors duration-300"
        >
          <div className="flex items-center gap-2">
            <span
              className={`font-gauge text-xs ${e.direction === "send" ? "text-ember" : "text-sonar"}`}
            >
              {e.direction === "send" ? "▲" : "▼"}
            </span>
            <span className="min-w-0 flex-1 truncate text-sm">
              {e.direction === "send" ? "发往 " : "来自 "}
              <span className="text-fog">{e.peerName}</span>
            </span>
            <span className={`gauge-label ${STATUS_META[e.status].color}`}>
              {STATUS_META[e.status].label}
            </span>
          </div>
          <div className="mt-1.5 flex items-center gap-3">
            <span className="font-gauge text-[11px] text-mist">
              {e.filesDone} 个文件 · {humanBytes(e.bytes)} · {shortTime(e.at)}
            </span>
            <span className="flex-1" />
            {e.lastPath && (
              <button
                onClick={() => revealItemInDir(e.lastPath ?? "")}
                className="cursor-pointer rounded border border-line-2 px-2 py-0.5 text-[11px] text-fog/80 transition-colors hover:border-sonar/50 hover:text-sonar"
              >
                显示
              </button>
            )}
          </div>
        </div>
      ))}
    </>
  );
}
