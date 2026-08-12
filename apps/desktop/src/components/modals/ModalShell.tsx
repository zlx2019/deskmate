// Shared dialog shell and controls used by all modal components.

import { Button as AButton, Switch as ASwitch } from "animal-island-ui";
import { useI18n } from "../../i18n";

/** Viewport-constrained dialog shell with internal scrolling for tall content. */
export function ModalShell({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose?: () => void;
  children: React.ReactNode;
}) {
  return (
    <div
      className="anim-fade-in fixed inset-0 z-50 flex items-center justify-center bg-black/35"
      onClick={onClose}
    >
      <div
        className="anim-fade-up flex max-h-[88vh] w-[440px] max-w-[92vw] flex-col overflow-hidden rounded-3xl border-2 border-line bg-panel shadow-[0_8px_24px_rgba(41,71,51,0.2)]"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Centered title with a close action in the top-right corner. */}
        <div className="relative flex shrink-0 items-center justify-center px-5 pt-4 pb-1 capitalize">
          <span className="text-[15px] font-bold text-fog">{title}</span>
          {onClose && (
            <button
              onClick={onClose}
              className="absolute top-1/2 right-4 flex size-7 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full bg-panel-2 text-mist transition-colors hover:text-fog"
            >
              ✕
            </button>
          )}
        </div>
        <div className="min-h-0 overflow-y-auto">{children}</div>
      </div>
    </div>
  );
}

/** Button wrapper preserving the existing primary and secondary variants. */
export function Button({
  children,
  onClick,
  variant = "ghost",
  disabled,
}: {
  children: React.ReactNode;
  onClick: () => void;
  variant?: "primary" | "ghost" | "danger";
  disabled?: boolean;
}) {
  return (
    <AButton
      type={variant === "primary" ? "primary" : "default"}
      danger={variant === "danger"}
      size="small"
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </AButton>
  );
}

/** Toggle row with a label, optional hint, and library Switch.
 * Rows without hints stay compact; hinted rows use separated alignment. */
export function ToggleRow({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  const { t } = useI18n();
  const control = (
    <ASwitch
      checked={checked}
      onChange={onChange}
      checkedChildren={t.common.on}
      unCheckedChildren={t.common.off}
      aria-label={label}
    />
  );
  if (!hint) {
    return (
      <div className="mt-3 flex items-center gap-2.5">
        <span className="text-sm text-fog">{label}</span>
        {control}
      </div>
    );
  }
  return (
    <div className="mt-3 flex items-center gap-3">
      <div className="min-w-0 flex-1">
        <div className="text-sm text-fog">{label}</div>
        <div className="mt-0.5 text-[11px] text-mist">{hint}</div>
      </div>
      {control}
    </div>
  );
}
