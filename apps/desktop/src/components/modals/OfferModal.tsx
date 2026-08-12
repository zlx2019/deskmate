// Incoming-offer dialog with manifest, capacity preflight, and conflict choices.

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

/** Incoming-offer dialog with capacity and file-name conflict checks. */
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
  // A null saveDir uses the default download directory.
  const [saveDir, setSaveDir] = useState<string | null>(null);
  const [defaultDir, setDefaultDir] = useState("");
  const [policy, setPolicy] = useState<ConflictPolicy>("rename");
  const [precheck, setPrecheck] = useState<PrecheckDto | null>(null);
  // Temporary dialog choice when the configured policy is ask.
  const [askChoice, setAskChoice] = useState<"rename" | "overwrite">("rename");

  // Load the default directory and conflict policy when opened.
  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        setDefaultDir(s.downloadDir);
        setPolicy(s.conflictPolicy);
      })
      .catch(console.error);
  }, []);

  // Re-run capacity and conflict checks when opened or the directory changes.
  useEffect(() => {
    setPrecheck(null);
    api
      .precheckReceive(saveDir ?? undefined, offer.files.map((f) => f.relPath))
      .then(setPrecheck)
      .catch(() => setPrecheck(null));
  }, [saveDir, offer]);

  const conflicts = precheck?.conflicts ?? [];
  // A failed capacity query does not block receiving.
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

        <div className="mt-3 max-h-40 overflow-y-auto rounded-xl border-2 border-line bg-panel-2">
          {offer.files.map((f) => (
            <div
              key={f.fileId}
              className="flex items-center gap-3 border-b border-line/60 px-3 py-1.5 last:border-b-0"
            >
              <span className="min-w-0 flex-1 truncate font-gauge text-xs text-fog/90">
                {f.relPath}
              </span>
              <span className="font-gauge text-[11px] text-mist">{humanBytes(f.size)}</span>
            </div>
          ))}
        </div>

        {/* Save location and available disk space. */}
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

        {/* File-name conflicts, with an inline choice for the ask policy. */}
        {conflicts.length > 0 &&
          (policy === "ask" ? (
            <div className="mt-2 rounded-xl border-2 border-ember/50 bg-ember/8 px-3 py-2">
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
