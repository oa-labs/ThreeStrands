import { useRef, useState } from "react";
import { Modal } from "./AppChrome";
import type { FormattingShortcut } from "./richText";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

const PROMPTS = {
  link: { title: "Insert Link", label: "Link URL", initial: "https://", action: "Insert Link" },
  color: { title: "Text Color", label: "Color", initial: "#2563eb", action: "Apply Color" },
} as const;

/**
 * Asks for the value a formatting shortcut needs (a link URL or a color).
 * The desktop webview has no `window.prompt`, so this replaces it in-app.
 */
export function FormattingPromptDialog({
  kind,
  onSubmit,
  onClose,
}: {
  kind: NonNullable<FormattingShortcut["prompt"]>;
  onSubmit(value: string): void;
  onClose(): void;
}) {
  const prompt = PROMPTS[kind];
  const [value, setValue] = useState<string>(prompt.initial);
  const input = useRef<HTMLInputElement>(null);
  return <Modal title={prompt.title} initialFocusRef={input} onClose={onClose}>
    <form className="modal-form" onSubmit={(event) => {
      event.preventDefault();
      if (value.trim()) onSubmit(value.trim());
    }}>
      <label>{prompt.label}
        <input
          {...SEARCH_INPUT_ATTRIBUTES}
          ref={input}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          onFocus={(event) => event.currentTarget.select()}
        />
      </label>
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={!value.trim()}>{prompt.action}</button>
      </div>
    </form>
  </Modal>;
}
