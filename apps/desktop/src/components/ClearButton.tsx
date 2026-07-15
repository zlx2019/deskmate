// 清理操作小件: 二段确认的清空按钮 + 卡片右上角删除角标
// 供互传记录与文字消息共用

import { useEffect, useState } from "react";

/** 卡片右上角的悬浮删除角标(hover 卡片时显示; 卡片容器需 relative group) */
export function CardClose({ onClick }: { onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      title="删除"
      className="absolute -top-1.5 -right-1.5 hidden size-[18px] cursor-pointer items-center justify-center rounded-full border border-line-2 bg-panel-2 text-mist transition-colors group-hover:flex hover:border-alert/60 hover:text-alert"
    >
      <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round">
        <path d="M18 6 6 18M6 6l12 12" />
      </svg>
    </button>
  );
}

/** 清空按钮(垃圾桶图标 → 点击变"确认清空"红字) */
export function ClearButton({ title, onConfirm }: { title: string; onConfirm: () => void }) {
  const [arming, setArming] = useState(false);

  // 待确认态 3s 超时自动恢复
  useEffect(() => {
    if (!arming) return;
    const t = setTimeout(() => setArming(false), 3000);
    return () => clearTimeout(t);
  }, [arming]);

  if (arming) {
    return (
      <button
        onClick={() => {
          setArming(false);
          onConfirm();
        }}
        className="cursor-pointer rounded border border-alert/40 px-2 py-0.5 text-[11px] text-alert/90 transition-colors hover:bg-alert/10"
      >
        确认清空
      </button>
    );
  }
  return (
    <button
      onClick={() => setArming(true)}
      title={title}
      className="cursor-pointer text-mist transition-colors hover:text-alert"
    >
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <path d="M3 6h18" />
        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
        <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
      </svg>
    </button>
  );
}
