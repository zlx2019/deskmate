// 接收确认弹窗: 对方发来传输请求时展示文件清单、空间预检与同名冲突决策

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { useI18n } from "../../i18n";
import {
  humanBytes,
  type ConflictPolicy,
  type OfferDto,
  type PrecheckDto,
} from "../../types";
import { Avatar } from "../Radar";
import { Button, ModalShell } from "./ModalShell";

/** 接收确认弹窗: 对方发来传输请求, 附带空间预检与同名冲突决策 */
export function OfferModal({
  offer,
  avatarSrc,
  onRespond,
}: {
  offer: OfferDto;
  avatarSrc?: string;
  onRespond: (
    offer: OfferDto,
    accept: boolean,
    opts?: { saveDir?: string; overwrite?: boolean },
  ) => void;
}) {
  const { t } = useI18n();
  // saveDir 为 null 表示用默认下载目录
  const [saveDir, setSaveDir] = useState<string | null>(null);
  const [defaultDir, setDefaultDir] = useState("");
  const [policy, setPolicy] = useState<ConflictPolicy>("rename");
  const [precheck, setPrecheck] = useState<PrecheckDto | null>(null);
  // ask 策略下用户在弹窗内的临时选择
  const [askChoice, setAskChoice] = useState<"rename" | "overwrite">("rename");

  // 打开时读取设置(默认目录 + 冲突策略)
  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        setDefaultDir(s.downloadDir);
        setPolicy(s.conflictPolicy);
      })
      .catch(console.error);
  }, []);

  // 打开与更换保存目录时预检(空间 + 冲突)
  useEffect(() => {
    setPrecheck(null);
    api
      .precheckReceive(saveDir ?? undefined, offer.files.map((f) => f.relPath))
      .then(setPrecheck)
      .catch(() => setPrecheck(null));
  }, [saveDir, offer]);

  const conflicts = precheck?.conflicts ?? [];
  // 可用空间查询失败(freeBytes 为 null)时不阻塞接收
  const notEnough = precheck?.freeBytes != null && precheck.freeBytes < offer.totalSize;
  const overwrite = policy === "overwrite" || (policy === "ask" && askChoice === "overwrite");

  const pickDir = async () => {
    const dir = await open({ directory: true, title: t.offer.pickDirTitle });
    if (typeof dir === "string") setSaveDir(dir);
  };

  return (
    <ModalShell title={t.offer.title}>
      <div className="px-5 py-4">
        <div className="flex items-center gap-3">
          <Avatar
            name={offer.peerName}
            fingerprint={offer.peerFingerprint}
            size={40}
            avatar={offer.peerAvatar}
            src={avatarSrc}
          />
          <div className="min-w-0">
            <div className="truncate text-sm text-fog">
              <span className="font-medium">{offer.peerName}</span> {t.offer.wantsToSend}
            </div>
            <div className="gauge-label mt-0.5">
              {t.offer.filesSummary(offer.files.length, humanBytes(offer.totalSize))}
            </div>
          </div>
        </div>

        <div className="mt-3 max-h-40 overflow-y-auto rounded-md border border-line/70 bg-abyss/50">
          {offer.files.map((f) => (
            <div
              key={f.fileId}
              className="flex items-center gap-3 border-b border-line/40 px-3 py-1.5 last:border-b-0"
            >
              <span className="min-w-0 flex-1 truncate font-gauge text-xs text-fog/90">
                {f.relPath}
              </span>
              <span className="font-gauge text-[11px] text-mist">{humanBytes(f.size)}</span>
            </div>
          ))}
        </div>

        {/* 保存位置 + 磁盘可用空间 */}
        <div className="mt-3 flex items-center gap-2">
          <span className="gauge-label shrink-0">{t.offer.saveTo}</span>
          <span className="min-w-0 flex-1 truncate font-gauge text-xs text-fog/90">
            {saveDir ?? defaultDir}
          </span>
          <button
            onClick={pickDir}
            className="shrink-0 cursor-pointer text-xs text-sonar transition-colors hover:text-fog"
          >
            {t.offer.change}
          </button>
        </div>
        {precheck?.freeBytes != null && (
          <div className={`mt-1 text-xs ${notEnough ? "text-alert" : "text-mist"}`}>
            {notEnough
              ? t.offer.notEnough(humanBytes(precheck.freeBytes), humanBytes(offer.totalSize))
              : t.offer.freeSpace(humanBytes(precheck.freeBytes))}
          </div>
        )}

        {/* 同名冲突: 按设置提示; "每次询问"时在此处当场选择 */}
        {conflicts.length > 0 &&
          (policy === "ask" ? (
            <div className="mt-2 rounded-md border border-ember/40 bg-ember/5 px-3 py-2">
              <div className="text-xs text-ember">{t.offer.conflictAsk(conflicts.length)}</div>
              <div className="mt-1.5 flex gap-4">
                {(
                  [
                    ["rename", t.offer.conflictRename],
                    ["overwrite", t.offer.conflictOverwrite],
                  ] as const
                ).map(([value, label]) => (
                  <label
                    key={value}
                    className="flex cursor-pointer items-center gap-1.5 text-xs text-fog/90"
                  >
                    <input
                      type="radio"
                      name="conflict-choice"
                      checked={askChoice === value}
                      onChange={() => setAskChoice(value)}
                      className="accent-(--color-sonar)"
                    />
                    {label}
                  </label>
                ))}
              </div>
            </div>
          ) : (
            <div className={`mt-1 text-xs ${overwrite ? "text-ember" : "text-mist"}`}>
              {t.offer.conflictNotice(conflicts.length, overwrite)}
            </div>
          ))}

        <div className="mt-4 flex items-center justify-end gap-2">
          <Button variant="danger" onClick={() => onRespond(offer, false)}>
            {t.offer.reject}
          </Button>
          <Button
            variant="primary"
            disabled={notEnough}
            onClick={() =>
              onRespond(offer, true, { saveDir: saveDir ?? undefined, overwrite })
            }
          >
            {t.offer.accept}
          </Button>
        </div>
      </div>
    </ModalShell>
  );
}
