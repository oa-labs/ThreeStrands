import frozenPolicy from "../docs/agent-integrations/contract-v1.json";
import { useEffect, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import type { AgentResearchConfig, AgentResearchSettings as Settings } from "./domain";
import { errorMessage } from "./errors";

export function AgentResearchSettings({ onClose, onSaved }: {onClose(): void; onSaved(): void}) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [modelKey, setModelKey] = useState("");
  const [searchKey, setSearchKey] = useState("");
  const policy=settings?.policy ?? frozenPolicy;
  const focus = useRef<HTMLInputElement>(null);
  useEffect(() => { let active = true; void mailClient.agentResearchSettings().then(s => {if(active) setSettings(s);}).catch(e => {if(active) setError(errorMessage(e));}); return () => {active=false;}; }, []);
  const patch = (p: Partial<AgentResearchConfig>) => setSettings(s => s ? {...s,config:{...s.config,...p,verified:false}} : s);
  const run = async (operation: () => Promise<void>) => {
    if(busy) return; setBusy(true); setError("");
    try {await operation();setSettings(await mailClient.agentResearchSettings());onSaved();} catch(e) {setError(errorMessage(e));} finally {setBusy(false);}
  };
  return <Modal title="Research settings" className="agent-research-settings" initialFocusRef={focus} onClose={onClose}>
    <form onSubmit={e => {e.preventDefault(); if(settings) void run(async () => {
      await mailClient.setAgentResearchSettings(settings.config);
      if(modelKey) {await mailClient.setAgentResearchKey("model",modelKey);setModelKey("");}
      if(searchKey) {await mailClient.setAgentResearchKey("search",searchKey);setSearchKey("");}
    });}}>
      <p className="agent-hint">Research uses Anthropic Claude Sonnet 4.6 and optional paid Exa search with separate keys in your system keychain. Existing AI settings are independent. Jobs run while this app is open and awake.</p>
      <p className="agent-hint">Your objective, public subjects, follow-up messages, and retrieved evidence go to Anthropic. Exa receives only reviewed public subjects with fixed research terms. Reading a page contacts its host using your IP address. Providers retain data under your account agreement; this is not a zero-retention promise.</p>
      <p className="agent-hint">Limits per task: {policy.limits.modelCallsPerTask} model calls, {policy.limits.searchCallsPerTask} searches, {policy.limits.pageCallsPerTask} pages, {policy.limits.inputTokensPerTask.toLocaleString()} input and {policy.limits.outputTokensPerTask.toLocaleString()} output tokens, and {policy.limits.activeSecondsPerTask/60} active minutes. Retries and revisions share that allowance. Estimates ({policy.verifiedOn}): ${policy.model.inputUsdPerMillion}/${policy.model.outputUsdPerMillion} per million input/output model tokens and ${policy.search.usdPerRequest} per search; actual bills can differ. No exact dollar cap is promised.</p>
      {error ? <p role="alert">{error}</p> : null}
      {settings ? <>
        <label className="agent-toggle"><input ref={focus} type="checkbox" checked={settings.config.enabled} onChange={e=>patch({enabled:e.target.checked})} />Enable web research</label>
        <label className="agent-toggle"><input type="checkbox" checked={settings.config.searchEnabled} onChange={e=>patch({searchEnabled:e.target.checked})} />Use paid Exa search</label>
        <label>Anthropic API key<input type="password" autoComplete="new-password" value={modelKey} onChange={e=>setModelKey(e.target.value)} placeholder={settings.modelKeyConfigured ? "Saved in keychain" : "Enter a separate research key"} /></label>
        <label>Exa API key<input type="password" autoComplete="new-password" value={searchKey} onChange={e=>setSearchKey(e.target.value)} placeholder={settings.searchKeyConfigured ? "Saved in keychain" : "Enter your Exa key"} /></label>
        <label>Agreement basis for saving Exa results<textarea value={settings.config.persistenceBasis} onChange={e=>patch({persistenceBasis:e.target.value})} aria-describedby="agent-persistence-help" /></label>
        <p className="agent-hint" id="agent-persistence-help">Record the applicable API/customer agreement allowing saved metadata, excerpts and reports. A working API key alone does not establish storage rights. Raw evidence expires after seven days; reports and cited excerpts remain until deletion. Backups and providers may retain copies.</p>
        <p role="status">{settings.config.verified ? "Connections checked for this configuration." : "Save changes, then check connections before starting research."}</p>
        <div className="agent-actions"><button className="btn btn-primary" disabled={busy}>Save research settings</button><button type="button" className="btn" disabled={busy || !settings.modelKeyConfigured || !settings.searchKeyConfigured} onClick={()=>void run(()=>mailClient.checkAgentResearchConnection())}>Check connections</button></div>
        <p className="agent-hint">Check connections sends one public test search to Exa, which may cost ${policy.search.usdPerRequest}, and checks model availability at Anthropic. It does not read your mail.</p>
        <div className="agent-actions"><button type="button" className="btn" disabled={busy || !settings.modelKeyConfigured} onClick={()=>void run(()=>mailClient.setAgentResearchKey("model",""))}>Disconnect Anthropic</button><button type="button" className="btn" disabled={busy || !settings.searchKeyConfigured} onClick={()=>void run(()=>mailClient.setAgentResearchKey("search",""))}>Disconnect Exa</button><button type="button" className="btn" onClick={onClose}>Close</button></div>
      </> : <p>Loading research settings…</p>}
    </form>
  </Modal>;
}
