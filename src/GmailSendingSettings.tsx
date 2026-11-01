import { useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { SendingIdentity } from "./domain";
import { errorMessage } from "./errors";

export function GmailSendingSettings({ account }: { account: string }) {
  const [identities, setIdentities] = useState<SendingIdentity[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void mailClient.listIdentities(account).then((list) => { if (active) setIdentities(list); }).catch((reason) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, [account]);
  const refresh = async () => {
    setBusy(true); setError("");
    try { setIdentities(await mailClient.refreshGmailAliases(account)); window.dispatchEvent(new CustomEvent("threestrands:sending-identities-changed", { detail: account })); }
    catch (reason) { setError(errorMessage(reason)); }
    finally { setBusy(false); }
  };
  return <section className="settings-subsection" aria-label={`Sending addresses for ${account}`}>
    <h4>Sending addresses</h4>
    <p className="settings-hint" id={`gmail-aliases-${account}`}>Add or verify addresses in Gmail’s Settings → Accounts → Send mail as, then refresh here. Only verified aliases are available for sending. Saved addresses are available offline.</p>
    <ul>{identities.map((identity) => <li key={identity.address}>{identity.address}{identity.isDefault ? " · Default for new messages" : ""}{identity.replyTo ? ` · Replies to ${identity.replyTo}` : ""}</li>)}</ul>
    <button type="button" className="btn btn-sm" disabled={busy} aria-describedby={`gmail-aliases-${account}`} onClick={() => { void refresh(); }}>{busy ? "Refreshing…" : "Refresh from Gmail"}</button>
    {error ? <p className="settings-error" role="alert">{error}</p> : null}
  </section>;
}
