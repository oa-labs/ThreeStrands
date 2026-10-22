import { useEffect, useRef, type ReactNode } from "react";

export type InlineConfirmAction = {
  label: string;
  onClick(): void;
  className?: string;
};

export function InlineConfirm({
  ariaLabel,
  title,
  children,
  cancelLabel,
  onCancel,
  actions,
  disabled = false,
  stacked = false,
  focusCancel = false,
}: {
  ariaLabel: string;
  /** The question being confirmed, shown above `children` as the explanation. */
  title?: ReactNode;
  children: ReactNode;
  cancelLabel: string;
  onCancel(): void;
  actions: InlineConfirmAction[];
  disabled?: boolean;
  /** Puts the buttons under the text, for narrow panes where side-by-side squeezes the message. */
  stacked?: boolean;
  /** Starts keyboard focus on the safe choice so Enter or Space never confirms by accident. */
  focusCancel?: boolean;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (focusCancel) cancelRef.current?.focus();
  }, [focusCancel]);
  return (
    <div className={`settings-inline-confirm notice--error${stacked ? " settings-inline-confirm--stacked" : ""}`} role="group" aria-label={ariaLabel}>
      {title ? (
        <div className="settings-inline-confirm-text">
          <p className="settings-inline-confirm-title">{title}</p>
          <p className="settings-inline-confirm-detail">{children}</p>
        </div>
      ) : <p>{children}</p>}
      <span className="settings-inline-confirm-actions">
        <button ref={cancelRef} className="btn" type="button" disabled={disabled} onClick={onCancel}>{cancelLabel}</button>
        {actions.map((action) => (
          <button key={action.label} type="button" className={action.className ? `btn ${action.className}` : "btn"} disabled={disabled} onClick={action.onClick}>
            {action.label}
          </button>
        ))}
      </span>
    </div>
  );
}
