// Right panel with transfer/history tabs, text-message stream, and composer.

import { memo, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Card as ACard,
  Icon as AIcon,
  Progress as AProgress,
  Tabs as ATabs,
  Tag as ATag,
  type TagColor,
} from "animal-island-ui";
// Settings action uses the item-001 wand icon from the component library.
import settingsIconUrl from "animal-island-ui/items/item-001.png";
import { api } from "../api";
import { useI18n } from "../i18n";
import { humanBytes, type PeerDto, type TextMsg, type TransferItem } from "../types";
import { CardClose, ClearButton } from "./ClearButton";
import { HistoryList } from "./HistoryList";
import { MessageComposer } from "./MessageComposer";

/** Maps status to the library Tag color. Labels resolve from the current locale;
 * index.css renders the default variant as muted green for neutral states. */
export const STATUS_TAG: Record<TransferItem["status"], TagColor> = {
  active: "app-orange",
  paused: "default",
  completed: "app-teal",
  cancelled: "default",
  interrupted: "app-red",
  rejected: "app-red",
  ignored: "default",
};

/** Status tag shared by transfer entries and history. */
export function StatusTag({ status, label }: { status: TransferItem["status"]; label: string }) {
  return (
    <ATag size="small" variant="soft" color={STATUS_TAG[status]}>
      {label}
    </ATag>
  );
}

/** Formats an ETA, for example 75 seconds as "1m 15s". */
function humanEta(seconds: number): string {
  const s = Math.round(seconds);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/** One transfer entry. */
function TransferCard({
  item,
  onPause,
  onResume,
  onPinRetry,
}: {
  item: TransferItem;
  onPause: (transferId: string) => void;
  onResume: (transferId: string) => void;
  onPinRetry: (item: TransferItem) => void;
}) {
  const { t } = useI18n();
  const pct = item.size > 0 ? Math.min(100, (item.done / item.size) * 100) : 0;
  const running = item.status === "active" || item.status === "paused";
  // Hide the current-file ETA until a speed sample exists.
  const eta =
    item.status === "active" && item.speed > 0 && item.size > item.done
      ? humanEta((item.size - item.done) / item.speed)
      : null;

  return (
    <ACard pattern="app-green" className="transfer-card">
      <div className="flex items-center gap-2">
        <span className={`font-gauge text-xs ${item.direction === "send" ? "text-ember" : "text-sonar"}`}>
          {item.direction === "send" ? "▲" : "▼"}
        </span>
        <span className="min-w-0 flex-1 truncate text-sm">
          {item.direction === "send" ? t.transfer.sendTo : t.transfer.recvFrom}
          <span className="text-fog">{item.peerName}</span>
        </span>
        <StatusTag
          status={item.status}
          label={
            item.status === "paused" && item.pausedByPeer && !item.pausedLocal
              ? t.transfer.pausedByPeer
              : t.transfer.status[item.status]
          }
        />
      </div>

      <div className="mt-1.5 truncate font-gauge text-xs text-mist">{item.currentFile}</div>

      {running && (
        <>
          {/* Library striped progress bar, filtered to gray while paused. */}
          <div className={`mt-2 ${item.status === "paused" ? "opacity-60 saturate-0" : ""}`}>
            <AProgress percent={pct} size="small" showInfo={false} duration={0.2} />
          </div>
          <div className="mt-1.5 flex items-center gap-3">
            <span className="font-gauge text-[11px] text-mist">
              {pct.toFixed(0)}% · {humanBytes(item.speed)}/s
              {eta && ` · ${t.transfer.eta(eta)}`}
            </span>
            <span className="flex-1" />
            {/* Hide resume when only the peer paused because local resume has no effect. */}
            {item.status === "active" ? (
              <PanelButton onClick={() => onPause(item.transferId)}>
                {t.transfer.pause}
              </PanelButton>
            ) : item.pausedLocal ? (
              <PanelButton onClick={() => onResume(item.transferId)}>
                {t.transfer.resume}
              </PanelButton>
            ) : null}
            <PanelButton danger onClick={() => api.cancel(item.transferId)}>
              {t.transfer.cancel}
            </PanelButton>
          </div>
        </>
      )}

      {item.status === "completed" && (
        <div className="mt-1.5 flex items-center gap-3">
          <span className="font-gauge text-[11px] text-mist">
            {t.transfer.files(item.filesDone)}
          </span>
          <span className="flex-1" />
          {item.lastPath && (
            <PanelButton onClick={() => revealItemInDir(item.lastPath ?? "")}>
              {t.transfer.reveal}
            </PanelButton>
          )}
        </div>
      )}

      {(item.status === "interrupted" ||
        item.status === "rejected" ||
        item.status === "ignored") && (
        <div className="mt-1.5 flex items-center gap-3">
          <span
            className={`min-w-0 flex-1 truncate text-[11px] ${
              item.status === "ignored" ? "text-mist" : "text-alert/80"
            }`}
            title={item.reason}
          >
            {item.reason ?? ""}
          </span>
          {/* Interrupted sends can resume; receivers wait for the sender. */}
          {item.status === "interrupted" && item.direction === "send" && (
            <PanelButton onClick={() => api.resumeSend(item.transferId)}>
              {t.transfer.resumeSend}
            </PanelButton>
          )}
          {/* Retry the original task after entering the peer's pairing PIN. */}
          {item.status === "rejected" && item.pinRequired && (
            <PanelButton onClick={() => onPinRetry(item)}>{t.transfer.enterPin}</PanelButton>
          )}
        </div>
      )}
    </ACard>
  );
}

/** Compact panel action button. */
function PanelButton({
  children,
  onClick,
  danger,
}: {
  children: React.ReactNode;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      className={`shrink-0 cursor-pointer rounded-full border-2 bg-panel px-2.5 py-0.5 text-[11px] font-bold whitespace-nowrap transition-colors ${
        danger
          ? "border-alert/50 text-alert hover:bg-alert/10"
          : "border-line text-fog/80 hover:border-sonar hover:text-sonar"
      }`}
    >
      {children}
    </button>
  );
}

/** Text-message card with direction label and hover delete action. */
function TextCard({ msg, onRemove }: { msg: TextMsg; onRemove: (id: string) => void }) {
  const { t } = useI18n();
  const out = msg.direction === "out";
  return (
    <ACard pattern="app-green" className="transfer-card group relative">
      <CardClose onClick={() => onRemove(msg.id)} />
      <div className="flex items-center gap-2">
        <span className={`font-gauge text-xs ${out ? "text-ember" : "text-sonar"}`}>✉</span>
        <span className="min-w-0 flex-1 truncate text-sm">
          {out ? t.transfer.sendTo : t.transfer.recvFrom}
          <span className="text-fog">{msg.peerName}</span>
        </span>
        <PanelButton onClick={() => navigator.clipboard.writeText(msg.text)}>
          {t.transfer.copy}
        </PanelButton>
      </div>
      {/* pre-wrap preserves the original whitespace and line breaks. */}
      <div className="mt-1.5 max-h-28 select-text overflow-auto whitespace-pre-wrap break-all rounded-xl border border-line bg-panel px-2.5 py-1.5 font-gauge text-xs text-fog/90">
        {msg.text}
      </div>
    </ACard>
  );
}

/** Tab count badge whose selected colors are controlled through aria-selected. */
function TabBadge({ count }: { count: number }) {
  return (
    <span className="tab-badge ml-1 rounded-full bg-chip px-2 py-px text-[11px] font-bold text-sonar">
      {count}
    </span>
  );
}

/** Memoized right panel that skips renders when non-progress props are unchanged. */
export const TransferPanel = memo(function TransferPanel({
  transfers,
  texts,
  peers,
  getPin,
  onPause,
  onResume,
  onPinLearned,
  onTextSent,
  onSendImage,
  onSendFiles,
  onRemoveText,
  onClearTexts,
  onPinRetry,
  onOpenSettings,
}: {
  transfers: TransferItem[];
  texts: TextMsg[];
  /** Online peers available to the composer. */
  peers: PeerDto[];
  getPin: (fingerprint: string) => string | undefined;
  /** Pauses or resumes a transfer and synchronizes entry state on success. */
  onPause: (transferId: string) => void;
  onResume: (transferId: string) => void;
  onPinLearned: (fingerprint: string, pin: string) => void;
  /** Records successfully sent text in the message stream. */
  onTextSent: (peerName: string, text: string) => void;
  /** Sends a clipboard screenshot from the global-hotkey flow. */
  onSendImage: (peer: PeerDto, fileName: string, bytes: Uint8Array) => Promise<void>;
  /** Sends copied clipboard files from the global-hotkey flow. */
  onSendFiles: (peer: PeerDto, paths: string[]) => Promise<void>;
  /** Deletes one text message. */
  onRemoveText: (id: string) => void;
  /** Clears all text messages. */
  onClearTexts: () => void;
  onPinRetry: (item: TransferItem) => void;
  /** Opens settings from the action at the right edge of the tab row. */
  onOpenSettings: () => void;
}) {
  const { t } = useI18n();
  // Upper tabs switch between transfers and history, remounting history to reload it.
  const [tab, setTab] = useState<"tasks" | "history">("tasks");
  const ordered = [...transfers].sort((a, b) => b.startedAt - a.startedAt);
  return (
    <div className="relative flex h-full flex-col overflow-hidden">
      {/* Settings action positioned at the tab row edge because Tabs has no extra slot. */}
      <button
        onClick={onOpenSettings}
        title={t.header.settings}
        className="absolute top-2 right-3 z-10 flex size-8 cursor-pointer items-center justify-center rounded-full transition-all hover:bg-panel-2 hover:brightness-105 active:scale-95"
      >
        <AIcon src={settingsIconUrl} size={22} bounce />
      </button>
      {/* Library Tabs renders only the selected page, remounting HistoryList to reload.
          The .transfer-tabs overrides integrate it with the panel layout. */}
      <ATabs
        className="dm-tabs transfer-tabs min-h-0 flex-[3]"
        activeKey={tab}
        onChange={(k) => setTab(k as "tasks" | "history")}
        aria-label={t.transfer.tabTasks}
        items={[
          {
            key: "tasks",
            label: (
              <>
                {t.transfer.tabTasks}
                <TabBadge count={ordered.length} />
              </>
            ),
            children: (
              <div className="flex flex-col gap-2.5 px-3 py-3">
                {ordered.length === 0 ? (
                  <div className="px-4 py-8 text-center text-xs text-mist/70">
                    {t.transfer.emptyTasks}
                  </div>
                ) : (
                  ordered.map((t) => (
                    <TransferCard
                      key={t.transferId}
                      item={t}
                      onPause={onPause}
                      onResume={onResume}
                      onPinRetry={onPinRetry}
                    />
                  ))
                )}
              </div>
            ),
          },
          {
            key: "history",
            label: t.transfer.tabHistory,
            children: (
              <div className="flex flex-col gap-2.5 px-3 py-3">
                <HistoryList />
              </div>
            ),
          },
        ]}
      />
      {/* Message section uses the same single-page Tabs style, with the clear
          action positioned at the tab row edge because no extra slot exists. */}
      <div className="relative flex min-h-0 flex-[2] flex-col border-t border-line">
        <ATabs
          className="dm-tabs transfer-tabs min-h-0 flex-1"
          activeKey="texts"
          aria-label={t.transfer.textSection}
          items={[
            {
              key: "texts",
              label: (
                <>
                  {t.transfer.textSection}
                  <TabBadge count={texts.length} />
                </>
              ),
              children: (
                <div className="flex flex-col gap-2.5 px-3 py-3">
                  {texts.length === 0 ? (
                    <div className="px-4 py-6 text-center text-xs text-mist/70">
                      {t.transfer.emptyTexts}
                    </div>
                  ) : (
                    texts.map((m) => <TextCard key={m.id} msg={m} onRemove={onRemoveText} />)
                  )}
                </div>
              ),
            },
          ]}
        />
        {texts.length > 0 && (
          <span className="absolute top-2.5 right-3 z-10">
            <ClearButton title={t.transfer.clearTexts} onConfirm={onClearTexts} />
          </span>
        )}
      </div>
      <MessageComposer
        peers={peers}
        getPin={getPin}
        onPinLearned={onPinLearned}
        onSent={onTextSent}
        onSendImage={onSendImage}
        onSendFiles={onSendFiles}
      />
    </div>
  );
});
