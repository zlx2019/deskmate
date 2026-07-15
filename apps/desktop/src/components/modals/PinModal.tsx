// PIN 输入弹窗: 发送被拒(对方要求配对 PIN)后输入 PIN 重试发送

import { useState } from "react";
import { useI18n } from "../../i18n";
import { Button, ModalShell } from "./ModalShell";

/** PIN 输入弹窗: 发送被拒(对方要求配对 PIN)后输入重试 */
export function PinModal({
  peerName,
  onSubmit,
  onClose,
}: {
  peerName: string;
  onSubmit: (pin: string) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [pin, setPin] = useState("");
  return (
    <ModalShell title={t.pinModal.title} onClose={onClose}>
      <div className="px-5 py-4">
        <div className="text-sm text-fog">
          <span className="font-medium">{peerName}</span>
          {t.pinModal.promptSuffix}
        </div>
        <input
          autoFocus
          value={pin}
          onChange={(e) => setPin(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && pin) onSubmit(pin);
          }}
          placeholder={t.pinModal.placeholder}
          className="mt-3 w-full rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 text-center font-gauge text-lg tracking-[0.3em] text-fog outline-none focus:border-sonar/60"
        />
        <div className="mt-4 flex items-center justify-end gap-2">
          <Button onClick={onClose}>{t.pinModal.cancel}</Button>
          <Button variant="primary" disabled={pin.length === 0} onClick={() => onSubmit(pin)}>
            {t.pinModal.retry}
          </Button>
        </div>
      </div>
    </ModalShell>
  );
}
