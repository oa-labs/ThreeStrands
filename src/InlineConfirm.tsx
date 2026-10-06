import type { ReactNode } from "react";

export type InlineConfirmAction = {
  label: string;
  onClick(): void;
  className?: string;
};

export function InlineConfirm({
  ariaLabel,
  children,
  cancelLabel,
  onCancel,
  actions,
  disabled = false,
}: {
  ariaLabel: string;
  children: ReactNode;
  cancelLabel: string;
  onCancel(): void;
  actions: InlineConfirmAction[];
  disabled?: boolean;
}) {
  return (
    <div className="settings-inline-confirm notice--error" role="group" aria-label={ariaLabel}>
      <p>{children}</p>
      <span className="settings-inline-confirm-actions">
        <button className="btn" type="button" disabled={disabled} onClick={onCancel}>{cancelLabel}</button>
        {actions.map((action) => (
          <button key={action.label} type="button" className={action.className ? `btn ${action.className}` : "btn"} disabled={disabled} onClick={action.onClick}>
            {action.label}
          </button>
        ))}
      </span>
    </div>
  );
}
