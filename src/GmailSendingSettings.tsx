import { useEffect, useId, useState } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import type { SendingIdentity } from "./domain";
import { errorMessage } from "./errors";

export function GmailSendingSettings({ account, onClose }: { account: string; onClose(): void }) {
  const helpId = useId();
  const [identities, setIdentities] = useState<SendingIdentity[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void mailClient.listIdentities(account)
      .then((list) => { if (active) setIdentities(list); })
      .catch((reason) => { if (active) setError(errorMessage(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [account]);
  const refresh = async () => {
    setBusy(true); setError("");
    try { setIdentities(await mailClient.refreshGmailAliases(account)); window.dispatchEvent(new CustomEvent("threestrands:sending-identities-changed", { detail: account })); }
    catch (reason) { setError(errorMessage(reason)); }
    finally { setBusy(false); }
  };
  const hasAliases = identities.some(identity => identity.address.trim().toLowerCase() !== account.trim().toLowerCase());
  const primaryReplyTo = !hasAliases ? identities.find(identity => identity.replyTo && identity.replyTo.trim().toLowerCase() !== account.trim().toLowerCase())?.replyTo : undefined;
  return <Modal title="Sending Addresses" className="sending-address-dialog" onClose={onClose}>
    <div className="sending-address-details">
      <p>{account}</p>
      {primaryReplyTo ? <p className="settings-hint">Replies to {primaryReplyTo}</p> : null}
      <p className="settings-hint" id={helpId}>Add or verify addresses in Gmail’s Settings → Accounts → Send mail as, then refresh here. Only verified aliases are available for sending. Saved addresses are available offline.</p>
      {hasAliases ? <ul className="sending-identity-list">{identities.map((identity) => <li key={identity.address}>
        <span>{identity.address}</span>
        {identity.isDefault || identity.replyTo ? <span className="sending-identity-detail">{[identity.isDefault ? "Default for new messages" : "", identity.replyTo ? `Replies to ${identity.replyTo}` : ""].filter(Boolean).join(" · ")}</span> : null}
      </li>)}</ul> : null}
      <button type="button" className="btn btn-sm" disabled={loading || busy} aria-describedby={helpId} onClick={() => { void refresh(); }}>{busy ? "Refreshing…" : "Refresh from Gmail"}</button>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </div>
  </Modal>;
}
