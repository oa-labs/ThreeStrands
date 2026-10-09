import { useState } from "react";
import type { Message } from "./domain";
import { Modal } from "./AppChrome";
import { parseAddress } from "./emailAddress";

export function UnsubscribeConfirm({
  message,
  onClose,
  onConfirm,
}: {
  message: Message;
  onClose(): void;
  onConfirm(): Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const method = message.unsubscribe?.methods[0];
  const sender = parseAddress(message.sender);
  const action = method === "oneClick"
    ? "Send One-Click Request"
    : method === "mailto"
      ? "Open Unsubscribe Email"
      : "Open Unsubscribe Page";

  return (
    <Modal title="Unsubscribe" className="unsubscribe-modal" onClose={onClose}>
      <div className="unsubscribe-content">
        <p>
          Unsubscribe from <strong>{sender.name}</strong>?
          {message.unsubscribe?.listId ? <span className="unsubscribe-list">{message.unsubscribe.listId}</span> : null}
        </p>
        <p className="unsubscribe-explanation">
          {method === "oneClick"
            ? "ThreeStrands will send the sender's one-click request without opening a web page."
            : method === "mailto"
              ? "ThreeStrands will start a new email addressed to the sender's unsubscribe address. You will still need to send it."
              : "ThreeStrands will open the sender's unsubscribe page in your default browser."}
        </p>
        <div className="unsubscribe-actions">
          <button type="button" className="btn" onClick={onClose} disabled={busy}>Cancel</button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy || !method}
            onClick={() => {
              setBusy(true);
              void onConfirm().finally(() => setBusy(false));
            }}
          >
            {busy ? "Working…" : action}
          </button>
        </div>
      </div>
    </Modal>
  );
}
