// 传输历史弹窗: 展示历史传输记录(方向/状态/字节数), 支持在文件管理器中显示

import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../../api";
import { humanBytes, type HistoryEntry } from "../../types";
import { Button, ModalShell } from "./ModalShell";

/** 传输历史弹窗 */
export function HistoryModal({ onClose }: { onClose: () => void }) {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);

  useEffect(() => {
    api.getHistory().then(setEntries).catch(() => setEntries([]));
  }, []);

  const statusLabel: Record<HistoryEntry["status"], string> = {
    completed: "已完成",
    cancelled: "已取消",
    interrupted: "已中断",
    rejected: "被拒绝",
  };

  return (
    <ModalShell title="transfer history" onClose={onClose}>
      <div className="max-h-[60vh] overflow-y-auto">
        {entries === null ? (
          <div className="py-8 text-center text-xs text-mist">加载中…</div>
        ) : entries.length === 0 ? (
          <div className="py-8 text-center text-xs text-mist">暂无传输记录</div>
        ) : (
          entries.map((e) => (
            <div
              key={`${e.transferId}-${e.at}`}
              className="flex items-center gap-3 border-b border-line/40 px-5 py-2.5 last:border-b-0"
            >
              <span
                className={`font-gauge text-xs ${e.direction === "send" ? "text-ember" : "text-sonar"}`}
              >
                {e.direction === "send" ? "▲" : "▼"}
              </span>
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm text-fog">{e.peerName}</div>
                <div className="gauge-label mt-0.5">
                  {e.filesDone} files · {humanBytes(e.bytes)} ·{" "}
                  {new Date(e.at).toLocaleString()}
                </div>
              </div>
              <span
                className={`shrink-0 text-xs ${
                  e.status === "completed"
                    ? "text-sonar"
                    : e.status === "cancelled"
                      ? "text-mist"
                      : "text-alert"
                }`}
              >
                {statusLabel[e.status]}
              </span>
              {e.lastPath && (
                <Button onClick={() => revealItemInDir(e.lastPath ?? "")}>显示</Button>
              )}
            </div>
          ))
        )}
      </div>
    </ModalShell>
  );
}
