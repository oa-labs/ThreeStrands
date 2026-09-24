import { Check, ListFilter, Search, X } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import {
  commands,
  shortcutSteps,
  type Command,
  type CommandContext,
} from "./commands";
import { MESSAGE_FILTER_OPTIONS, type MessageFilterKind } from "./messageFilters";
import { formattingShortcuts } from "./richText";
import { useEscapeDismiss } from "./useEscapeDismiss";

export function FiltersButton({
  activeFilters,
  onToggleFilter,
}: {
  activeFilters: Set<MessageFilterKind>;
  onToggleFilter(kind: MessageFilterKind): void;
}) {
  const [open, setOpen] = useState(false);
  const anchorRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (anchorRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [open]);

  return (
    <div className="filters-anchor" ref={anchorRef}>
      <button
        type="button"
        className={`filters-trigger ${activeFilters.size > 0 ? "active" : ""}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <ListFilter size={15} />
        <span>Filters</span>
        {activeFilters.size > 0 ? <span className="filters-badge">{activeFilters.size}</span> : null}
      </button>
      {open ? <FiltersMenu activeFilters={activeFilters} onToggleFilter={onToggleFilter} onClose={() => setOpen(false)} /> : null}
    </div>
  );
}

function FiltersMenu({
  activeFilters,
  onToggleFilter,
  onClose,
}: {
  activeFilters: Set<MessageFilterKind>;
  onToggleFilter(kind: MessageFilterKind): void;
  onClose(): void;
}) {
  useEscapeDismiss(onClose);
  return (
    <div className="filters-menu" role="menu" aria-label="Filters">
      <div className="filters-menu-title">Filters</div>
      {MESSAGE_FILTER_OPTIONS.map((option) => {
        const isActive = activeFilters.has(option.kind);
        return (
          <button
            key={option.kind}
            type="button"
            role="menuitemcheckbox"
            aria-checked={isActive}
            className={`filters-menu-item ${isActive ? "active" : ""}`}
            onClick={() => onToggleFilter(option.kind)}
          >
            <span className="filters-menu-item-label">
              <span className="filters-menu-item-check" aria-hidden="true">{isActive ? <Check size={13} /> : null}</span>
              {option.label}
            </span>
            <span className="filters-menu-item-keys"><kbd>shift</kbd><kbd>{option.shortcutKey}</kbd></span>
          </button>
        );
      })}
    </div>
  );
}

export function HoverTooltip({
  children,
  label,
  title,
  placement = "right",
  shortcut,
}: {
  children: ReactNode;
  label?: string;
  title?: string;
  placement?: "right" | "bottom";
  shortcut?: string;
}) {
  const tooltipLabel = label ?? title;
  if (!tooltipLabel) throw new Error("HoverTooltip requires a label or title");
  return (
    <span className={`tooltip-anchor tooltip-${placement}`} title={title}>
      {children}
      <span className="hover-tooltip" role="tooltip"><strong>{tooltipLabel}</strong>{shortcut ? <kbd>{shortcut}</kbd> : null}</span>
    </span>
  );
}

export function ActionButton({
  children,
  label,
  onClick,
  shortcut,
}: {
  children: ReactNode;
  label: string;
  onClick(): void;
  shortcut?: string;
}) {
  return <button className="action-button" aria-label={shortcut ? `${label} (${shortcut})` : label} onClick={onClick}>{children}<span>{label}</span>{shortcut ? <kbd>{shortcut}</kbd> : null}</button>;
}

export function CommandPalette({
  context,
  execute,
  extraCommands = [],
  onClose,
}: {
  context: CommandContext;
  execute(command: Command): void;
  extraCommands?: Command[];
  onClose(): void;
}) {
  const [filter, setFilter] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => inputRef.current?.focus(), []);
  const visible = [...commands, ...extraCommands].filter((command) => command.title.toLocaleLowerCase().includes(filter.toLocaleLowerCase()));
  return (
    <Modal title="Command Palette" onClose={onClose} shortcutScope="palette" initialFocusRef={inputRef}>
      <label className="palette-search">
        <Search size={18} />
        <input ref={inputRef} value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Type a command" aria-label="Filter Commands" />
      </label>
      <div className="command-list">
        {visible.map((command) => (
          <button key={command.id} disabled={!command.enabled(context)} onClick={() => { execute(command); onClose(); }}>
            <span><small>{command.group}</small>{command.title}</span>
            <span>{command.keys.map((key) => <ShortcutKeys key={key} shortcut={key} />)}</span>
          </button>
        ))}
      </div>
    </Modal>
  );
}

const shortcutGroupOrder = ["Navigation", "Triage", "Compose", "Application"] as const;

export function ShortcutHelp({ extraCommands = [], onClose }: { extraCommands?: Command[]; onClose(): void }) {
  const shortcutCommands = [
    ...[...commands, ...extraCommands].filter((command) => command.keys.length > 0),
    ...formattingShortcuts.map((shortcut) => ({ id: shortcut.id, title: shortcut.title, keys: [shortcut.key], group: "Compose" as const })),
  ];
  return (
    <Modal title="Keyboard Shortcuts" className="shortcut-help-modal" onClose={onClose}>
      <p className="shortcut-help-intro">Use ThreeStrands without leaving the keyboard.</p>
      <div className="shortcut-help-groups">
        {shortcutGroupOrder.map((group) => {
          const groupCommands = shortcutCommands.filter((command) => command.group === group);
          if (groupCommands.length === 0) return null;
          return (
            <section key={group} aria-labelledby={`shortcut-group-${group.toLowerCase()}`}>
              <h3 id={`shortcut-group-${group.toLowerCase()}`}>{group}</h3>
              <dl>{groupCommands.map((command) => <div key={command.id}><dt>{command.title}</dt><dd>{command.keys.map((key) => <ShortcutKeys key={key} shortcut={key} />)}</dd></div>)}</dl>
            </section>
          );
        })}
      </div>
    </Modal>
  );
}

/** Splits on "+" separators only, so a literal plus key ("Mod++") survives as its own part. */
function formatShortcutStep(step: string): string {
  return step.split(/\+(?=.)/).map((part) => (part === "Mod" ? "⌘/Ctrl" : part)).join(" + ");
}

function ShortcutKeys({ shortcut }: { shortcut: string }) {
  return <span className="shortcut-keys">{shortcutSteps(shortcut).map((step, index) => <span key={step}>{index > 0 ? <small>then</small> : null}<kbd>{formatShortcutStep(step)}</kbd></span>)}</span>;
}

export function Modal({
  className,
  children,
  dismissible = true,
  initialFocusRef,
  onClose,
  shortcutScope = "modal",
  title,
}: {
  className?: string;
  children: ReactNode;
  /** When false, Escape, a backdrop click, and the header close button do
   * nothing — the dialog's own content must offer the only way out. Escape
   * is still claimed so it cannot fall through to a modal underneath. */
  dismissible?: boolean;
  initialFocusRef?: RefObject<HTMLElement | null>;
  onClose(): void;
  shortcutScope?: "modal" | "palette";
  title: string;
}) {
  const backdropRef = useRef<HTMLDivElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(document.activeElement instanceof HTMLElement ? document.activeElement : null);
  const dismiss = dismissible ? onClose : () => {};
  useEscapeDismiss(dismiss);

  useEffect(() => {
    const backdrop = backdropRef.current;
    const previousFocus = previousFocusRef.current;
    // Never hide a modal stacked above this one: when two modals mount in
    // the same commit, the later backdrop already exists when this runs.
    const stackedAbove = (element: Element) =>
      element.classList.contains("modal-backdrop")
      && Boolean(backdrop && backdrop.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING);
    const background = [...document.body.children].filter((element) => element !== backdrop && !stackedAbove(element));
    const previousBackgroundState = background.map((element) => ({
      element,
      ariaHidden: element.getAttribute("aria-hidden"),
      inert: (element as HTMLElement).inert,
    }));
    for (const element of background) {
      element.setAttribute("aria-hidden", "true");
      (element as HTMLElement).inert = true;
    }

    const currentFocus = document.activeElement instanceof HTMLElement && dialogRef.current?.contains(document.activeElement)
      ? document.activeElement
      : null;
    const focusTarget = initialFocusRef?.current
      ?? currentFocus
      ?? dialogRef.current?.querySelector<HTMLElement>("[autofocus], [data-autofocus], input:not([disabled]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), [href], [tabindex]:not([tabindex='-1'])")
      ?? dialogRef.current;
    focusTarget?.focus();

    return () => {
      for (const { element, ariaHidden, inert } of previousBackgroundState) {
        if (ariaHidden === null) element.removeAttribute("aria-hidden");
        else element.setAttribute("aria-hidden", ariaHidden);
        (element as HTMLElement).inert = inert;
      }
      previousFocus?.focus({ preventScroll: true });
    };
  }, [initialFocusRef]);

  const trapFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Tab") return;
    const focusable = [...(dialogRef.current?.querySelectorAll<HTMLElement>(
      "button:not([disabled]), [href], input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex='-1'])",
    ) ?? [])].filter((element) => !element.hidden && element.getAttribute("aria-hidden") !== "true");
    if (focusable.length === 0) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable.at(-1)!;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return createPortal(
    <div ref={backdropRef} className="modal-backdrop" role="presentation" onMouseDown={dismiss}>
      <div
        ref={dialogRef}
        className={`modal${className ? ` ${className}` : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        data-shortcut-scope={shortcutScope}
        tabIndex={-1}
        onKeyDown={trapFocus}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header><h2>{title}</h2>{dismissible ? <button aria-label="Close" onClick={onClose}><X size={18} /></button> : null}</header>
        {children}
      </div>
    </div>,
    document.body,
  );
}
