import { Check, ListFilter, Search, X } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
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
  placement = "right",
  shortcut,
}: {
  children: ReactNode;
  label: string;
  placement?: "right" | "bottom";
  shortcut?: string;
}) {
  return (
    <span className={`tooltip-anchor tooltip-${placement}`}>
      {children}
      <span className="hover-tooltip" role="tooltip"><strong>{label}</strong>{shortcut ? <kbd>{shortcut}</kbd> : null}</span>
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
    <Modal title="Command palette" onClose={onClose}>
      <label className="palette-search">
        <Search size={18} />
        <input ref={inputRef} value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Type a command" aria-label="Filter commands" />
      </label>
      <div className="command-list">
        {visible.map((command) => (
          <button key={command.id} disabled={!command.enabled(context)} onClick={() => { execute(command); onClose(); }}>
            <span><small>{command.group}</small>{command.title}</span>
            <span>{command.keys.map((key) => <kbd key={key}>{key}</kbd>)}</span>
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
    <Modal title="Keyboard shortcuts" className="shortcut-help-modal" onClose={onClose}>
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

function ShortcutKeys({ shortcut }: { shortcut: string }) {
  return <span className="shortcut-keys">{shortcutSteps(shortcut).map((step, index) => <span key={step}>{index > 0 ? <small>then</small> : null}<kbd>{step.replace("Mod", "⌘/Ctrl").replaceAll("+", " + ")}</kbd></span>)}</span>;
}

export function Modal({
  className,
  children,
  onClose,
  title,
}: {
  className?: string;
  children: ReactNode;
  onClose(): void;
  title: string;
}) {
  useEscapeDismiss(onClose);
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <div className={`modal${className ? ` ${className}` : ""}`} role="dialog" aria-modal="true" aria-label={title} onMouseDown={(event) => event.stopPropagation()}>
        <header><h2>{title}</h2><button aria-label="Close" onClick={onClose}><X size={18} /></button></header>
        {children}
      </div>
    </div>
  );
}

