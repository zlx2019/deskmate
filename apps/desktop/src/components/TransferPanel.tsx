// 右侧面板: "传输任务 / 互传记录" tab + 文字消息流 + 聊天输入行

import { memo, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { humanBytes, type PeerDto, type TextMsg, type TransferItem } from "../types";
import { HistoryList } from "./HistoryList";
import { MessageComposer } from "./MessageComposer";

/** 状态视觉映射 */
const STATUS_META: Record<
  TransferItem["status"],
  { label: string; color: string }
> = {
  active: { label: "传输中", color: "text-ember" },
  paused: { label: "已暂停", color: "text-mist" },
  completed: { label: "已完成", color: "text-sonar" },
  cancelled: { label: "已取消", color: "text-mist" },
  interrupted: { label: "已中断", color: "text-alert" },
  rejected: { label: "被拒绝", color: "text-alert" },
};

/** 剩余时间估算: 75 → "1m 15s" */
function humanEta(seconds: number): string {
  const s = Math.round(seconds);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/** 单个传输条目卡片 */
function TransferCard({
  item,
  onPinRetry,
}: {
  item: TransferItem;
  onPinRetry: (item: TransferItem) => void;
}) {
  const meta = STATUS_META[item.status];
  const pct = item.size > 0 ? Math.min(100, (item.done / item.size) * 100) : 0;
  const running = item.status === "active" || item.status === "paused";
  // 当前文件的剩余时间(速度尚未采样时不显示)
  const eta =
    item.status === "active" && item.speed > 0 && item.size > item.done
      ? humanEta((item.size - item.done) / item.speed)
      : null;

  return (
    <div className="rounded-xl border border-line bg-panel-2 px-3 py-2.5 transition-colors duration-300">
      <div className="flex items-center gap-2">
        <span className={`font-gauge text-xs ${item.direction === "send" ? "text-ember" : "text-sonar"}`}>
          {item.direction === "send" ? "▲" : "▼"}
        </span>
        <span className="min-w-0 flex-1 truncate text-sm">
          {item.direction === "send" ? "发往 " : "来自 "}
          <span className="text-fog">{item.peerName}</span>
        </span>
        <span className={`gauge-label ${meta.color}`}>{meta.label}</span>
      </div>

      <div className="mt-1.5 truncate font-gauge text-xs text-mist">{item.currentFile}</div>

      {running && (
        <>
          <div className="mt-2 h-1 overflow-hidden rounded-full bg-line">
            <div
              className={`h-full rounded-full transition-[width] duration-200 ${
                item.status === "paused" ? "bg-mist" : "bg-ember"
              }`}
              style={{ width: `${pct}%` }}
            />
          </div>
          <div className="mt-1.5 flex items-center gap-3">
            <span className="font-gauge text-[11px] text-mist">
              {pct.toFixed(0)}% · {humanBytes(item.speed)}/s
              {eta && ` · 剩余 ${eta}`}
            </span>
            <span className="flex-1" />
            {item.status === "active" ? (
              <PanelButton onClick={() => api.pause(item.transferId)}>暂停</PanelButton>
            ) : (
              <PanelButton onClick={() => api.resume(item.transferId)}>继续</PanelButton>
            )}
            <PanelButton danger onClick={() => api.cancel(item.transferId)}>
              取消
            </PanelButton>
          </div>
        </>
      )}

      {item.status === "completed" && (
        <div className="mt-1.5 flex items-center gap-3">
          <span className="font-gauge text-[11px] text-mist">{item.filesDone} 个文件</span>
          <span className="flex-1" />
          {item.lastPath && (
            <PanelButton onClick={() => revealItemInDir(item.lastPath ?? "")}>
              显示
            </PanelButton>
          )}
        </div>
      )}

      {(item.status === "interrupted" || item.status === "rejected") && (
        <div className="mt-1.5 flex items-center gap-3">
          <span
            className="min-w-0 flex-1 truncate text-[11px] text-alert/80"
            title={item.reason}
          >
            {item.reason ?? ""}
          </span>
          {/* 发送侧中断可续传(补发缺失段); 接收侧被动等待对方续传 */}
          {item.status === "interrupted" && item.direction === "send" && (
            <PanelButton onClick={() => api.resumeSend(item.transferId)}>续传</PanelButton>
          )}
          {/* 对方要求配对 PIN: 输入后原任务重试 */}
          {item.status === "rejected" && item.pinRequired && (
            <PanelButton onClick={() => onPinRetry(item)}>输入 PIN</PanelButton>
          )}
        </div>
      )}
    </div>
  );
}

/** 面板内的小按钮 */
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
      className={`cursor-pointer rounded border px-2 py-0.5 text-[11px] transition-colors ${
        danger
          ? "border-alert/40 text-alert/90 hover:bg-alert/10"
          : "border-line-2 text-fog/80 hover:border-sonar/50 hover:text-sonar"
      }`}
    >
      {children}
    </button>
  );
}

/** 文本消息卡片(按方向区分"来自/发往") */
function TextCard({ msg }: { msg: TextMsg }) {
  const out = msg.direction === "out";
  return (
    <div className="rounded-xl border border-line bg-panel-2 px-3 py-2.5 transition-colors duration-300">
      <div className="flex items-center gap-2">
        <span className={`font-gauge text-xs ${out ? "text-ember" : "text-sonar"}`}>✉</span>
        <span className="min-w-0 flex-1 truncate text-sm">
          {out ? "发往 " : "来自 "}
          <span className="text-fog">{msg.peerName}</span>
        </span>
        <PanelButton onClick={() => navigator.clipboard.writeText(msg.text)}>复制</PanelButton>
      </div>
      {/* 逐字节原样展示: pre-wrap 保留空白与换行 */}
      <div className="mt-1.5 max-h-28 select-text overflow-auto whitespace-pre-wrap break-all rounded border border-line/60 bg-abyss/60 px-2.5 py-1.5 font-gauge text-xs text-fog/90">
        {msg.text}
      </div>
    </div>
  );
}

/** 顶部 tab 按钮(选中态带下划线) */
function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={`relative flex cursor-pointer items-center gap-2 px-3 py-2.5 text-xs font-medium tracking-[0.14em] transition-colors ${
        active ? "text-fog" : "text-mist hover:text-fog/80"
      }`}
    >
      {children}
      {active && <span className="absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-sonar" />}
    </button>
  );
}

/** 右侧栏(memo: 节点高频进度更新之外的 props 变化频率低, 未变即跳过) */
export const TransferPanel = memo(function TransferPanel({
  transfers,
  texts,
  peers,
  getPin,
  onPinLearned,
  onTextSent,
  onSendImage,
  onPinRetry,
}: {
  transfers: TransferItem[];
  texts: TextMsg[];
  /** 在线节点(聊天输入行的目标候选) */
  peers: PeerDto[];
  getPin: (fingerprint: string) => string | undefined;
  onPinLearned: (fingerprint: string, pin: string) => void;
  /** 文本发送成功(记入消息流) */
  onTextSent: (peerName: string, text: string) => void;
  /** 发送剪贴板截图(全局快捷键路径) */
  onSendImage: (peer: PeerDto, fileName: string, bytes: number[]) => Promise<void>;
  onPinRetry: (item: TransferItem) => void;
}) {
  // 上半区分页: 传输任务 / 互传记录(切回记录页时重新拉取)
  const [tab, setTab] = useState<"tasks" | "history">("tasks");
  const ordered = [...transfers].sort((a, b) => b.startedAt - a.startedAt);
  return (
    <div className="flex h-full flex-col overflow-hidden">
      <div className="flex items-center border-b border-line px-2">
        <TabButton active={tab === "tasks"} onClick={() => setTab("tasks")}>
          传输任务
          <span className="rounded-full bg-chip px-2 py-px text-[11px] font-medium text-sonar">
            {ordered.length}
          </span>
        </TabButton>
        <TabButton active={tab === "history"} onClick={() => setTab("history")}>
          互传记录
        </TabButton>
      </div>
      <div className="flex min-h-0 flex-[3] flex-col gap-2.5 overflow-y-auto px-3 py-3">
        {tab === "history" ? (
          <HistoryList />
        ) : ordered.length === 0 ? (
          <div className="px-4 py-8 text-center text-xs text-mist/70">
            拖拽文件到地图上的设备即可发送
          </div>
        ) : (
          ordered.map((t) => (
            <TransferCard key={t.transferId} item={t} onPinRetry={onPinRetry} />
          ))
        )}
      </div>
      <div className="flex items-center gap-2 border-y border-line px-4 py-2.5">
        <span className="text-xs font-medium tracking-[0.14em] text-mist">文字消息</span>
        <span className="rounded-full bg-chip px-2 py-px text-[11px] font-medium text-sonar">
          {texts.length}
        </span>
      </div>
      <div className="flex min-h-0 flex-[2] flex-col gap-2.5 overflow-y-auto px-3 py-3">
        {texts.length === 0 ? (
          <div className="px-4 py-6 text-center text-xs text-mist/70">暂无文本消息</div>
        ) : (
          texts.map((m) => <TextCard key={m.id} msg={m} />)
        )}
      </div>
      <MessageComposer
        peers={peers}
        getPin={getPin}
        onPinLearned={onPinLearned}
        onSent={onTextSent}
        onSendImage={onSendImage}
      />
    </div>
  );
});
