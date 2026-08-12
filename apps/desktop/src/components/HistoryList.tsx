// Transfer history tab content, reloaded whenever the component mounts.

import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Card as ACard } from "animal-island-ui";
import { api } from "../api";
import { useI18n } from "../i18n";
import { humanBytes, type HistoryEntry } from "../types";
import { CardClose, ClearButton } from "./ClearButton";
import { StatusTag } from "./TransferPanel";

/** Formats today's entries as time only and older entries with month and day. */
function shortTime(ts: number): string {
  const d = new Date(ts);
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  return d.toDateString() === new Date().toDateString()
    ? hm
    : `${d.getMonth() + 1}/${d.getDate()} ${hm}`;
}

/** Compact transfer-history list with per-entry deletion and clear-all. */
export function HistoryList() {
  const { t } = useI18n();
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);

  useEffect(() => {
    api.getHistory().then(setEntries).catch(() => setEntries([]));
  }, []);

  /** Removes locally first for immediate feedback, logging backend failure. */
  const removeOne = (transferId: string) => {
    setEntries((prev) => prev?.filter((e) => e.transferId !== transferId) ?? prev);
    api.deleteHistory(transferId).catch(console.error);
  };

  /** Clears all entries after ClearButton confirmation. */
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
        <ACard
          key={`${e.transferId}-${e.at}`}
          pattern="app-green"
          className="transfer-card group relative"
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
            <StatusTag status={e.status} label={t.transfer.status[e.status]} />
          </div>
          <div className="mt-1.5 flex items-center gap-3">
            <span className="font-gauge text-[11px] text-mist">
              {t.transfer.files(e.filesDone)} · {humanBytes(e.bytes)} · {shortTime(e.at)}
            </span>
            <span className="flex-1" />
            {e.lastPath && (
              <button
                onClick={() => revealItemInDir(e.lastPath ?? "")}
                className="shrink-0 cursor-pointer rounded-full border-2 border-line bg-panel px-2.5 py-0.5 text-[11px] whitespace-nowrap font-bold text-fog/80 transition-colors hover:border-sonar hover:text-sonar"
              >
                {t.transfer.reveal}
              </button>
            )}
          </div>
        </ACard>
      ))}
    </>
  );
}
