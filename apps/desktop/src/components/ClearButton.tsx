// 二段确认的清空按钮: 首次点击进入待确认态(3s 内再点执行, 超时恢复)
// 供互传记录与文字消息的"清空全部"共用, 避免破坏性操作误触

import { useEffect, useState } from "react";

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
