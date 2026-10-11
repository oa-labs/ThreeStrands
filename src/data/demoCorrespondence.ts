import { previewScheduleChoices } from "../scheduleTimes";
import type { CorrespondenceClient, Draft, OutboxItem, ScheduledSendReport, ScheduleSelection } from "../correspondence";
import { plainTextToHtml } from "../richText";
import type { ThreadDetail } from "../domain";

const key = "threestrands.demoCorrespondence";
const inlineImages = new Map<string, string>();
type Store = { drafts: Draft[]; outbox: OutboxItem[]; summaries?: ScheduledSendReport[] };
const previewOwner = "00000000-0000-4000-8000-000000000001";
function read(): Store {
  const saved = localStorage.getItem(key);
  return saved ? JSON.parse(saved) as Store : { drafts: [], outbox: [] };
}
/** Same readable shape the native draft builder writes ("Mon, Oct 5, 2026 at 9:32 AM"). */
function attributionDate(sentAt: string): string {
  const date = new Date(sentAt);
  if (Number.isNaN(date.getTime())) return sentAt;
  const day = date.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric", year: "numeric" });
  const time = date.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
  return `${day} at ${time}`;
}
function write(store: Store) { localStorage.setItem(key, JSON.stringify(store)); }
function draft(store: Store, id: string) {
  const result = store.drafts.find((d) => d.id === id);
  if (!result) throw new Error("Draft no longer exists");
  return result;
}
function tick(store: Store) {
  store.outbox.forEach((item) => {
    if (item.state === "scheduled" && item.deadline < Date.now() - 60_000) item.state = "overdue";
    if (item.state === "scheduled" && item.deadline <= Date.now() && navigator.onLine) item.state = "sent";
    if (item.schedule && item.schedule.report.state !== item.state) {
      item.schedule.report.state = item.state; item.schedule.report.reportRevision++; item.schedule.report.statusChangedAt = Date.now();
    }
  });
  if (navigator.onLine) store.outbox.forEach((item) => {
    if (item.state === "undo_pending" && item.deadline <= Date.now()) item.state = "sent";
  });
  write(store);
}
function cancel(id: string, recover = false) {
  const store = read(); tick(store);
  const item = store.outbox.find((o) => o.id === id);
  if (!item || !(recover ? item.state === "failed" : ["undo_pending", "ready", "scheduled", "overdue"].includes(item.state))) throw new Error("Delivery has already started");
  if (item.schedule && item.schedule.report.ownerInstallationId !== previewOwner) throw new Error("This message belongs to another sending computer");
  item.state = "canceled";
  if (item.schedule) { item.schedule.report.state = "canceled"; item.schedule.report.reportRevision++; }
  const restored = { ...item.draft, revision: item.draft.revision + 1 };
  store.drafts.push(restored); write(store); return restored;
}
function resolveSelection(selection: ScheduleSelection) {
  const choices = previewScheduleChoices(selection.localTime,selection.timeZone);
  const selected = choices.length === 1 && selection.offsetSeconds == null ? choices[0] : choices.find((v) => v.offsetSeconds === selection.offsetSeconds);
  if (!selected) throw new Error("This time occurs twice. Choose its UTC offset.");
  if (selected.scheduledAt <= Date.now()) throw new Error("Choose a time in the future");
  return selected.scheduledAt;
}
export function demoCorrespondence(getSourceThread: (messageId: string) => Promise<ThreadDetail>, defaultAccount: () => string): CorrespondenceClient {
  const recovered = read();
  recovered.outbox.forEach((item) => { if (item.state === "scheduled" && item.deadline < Date.now()) item.state = "overdue"; });
  write(recovered);
  return {
    async schedulingInfo() { return { deviceName: "This computer", sharing: false }; },
    async scheduleChoices(localTime,timeZone) { return previewScheduleChoices(localTime,timeZone); },
    async listScheduledSummaries() { return read().summaries ?? []; },
    async scheduleDraft(id,revision,selection) {
      const store=read();
      const queued=store.outbox.find((o) => o.draft.id===id && o.draft.revision===revision);
      if (queued) return queued;
      const d=draft(store,id);
      if (d.revision!==revision) throw new Error("Draft is still saving");
      if (![d.to,d.cc,d.bcc].some((v) => v.trim())) throw new Error("Add at least one recipient");
      if ([d.to,d.cc,d.bcc].some((v) => v.trim() && !v.includes("@"))) throw new Error("Enter a complete email address");
      if (d.attachments.some((a) => !a.ready)) throw new Error("Download or remove unavailable attachments");
      const scheduledAt=resolveSelection(selection), operationId=crypto.randomUUID();
      const item: OutboxItem={id:operationId,draft:d,state:"scheduled",deadline:scheduledAt,error:null,schedule:{canManage:true,visibility:"local",report:{operationId,ownerInstallationId:previewOwner,ownerSyncDeviceId:null,ownerNameAtCreation:"This computer",account:d.account,subject:d.subject,scheduledAt,timeZone:selection.timeZone,state:"scheduled",blockedReason:null,reportRevision:1,statusChangedAt:Date.now()}}};
      store.outbox.push(item); store.drafts=store.drafts.filter((v) => v.id!==id); write(store); return item;
    },
    async rescheduleSend(id,revision,selection) {
      const store=read(); tick(store); const item=store.outbox.find((o) => o.id===id);
      if (!item?.schedule || item.schedule.report.ownerInstallationId!==previewOwner || item.schedule.report.reportRevision!==revision || !["scheduled","overdue"].includes(item.state)) throw new Error("Schedule changed or belongs to another computer");
      const at=resolveSelection(selection); item.state="scheduled";item.deadline=at;item.schedule.report.scheduledAt=at;item.schedule.report.timeZone=selection.timeZone;item.schedule.report.state="scheduled";item.schedule.report.reportRevision++;item.schedule.report.statusChangedAt=Date.now(); write(store);
    },
    async sendScheduledNow(id,revision) {
      const store=read(); const item=store.outbox.find((o) => o.id===id);
      if (!item?.schedule || item.schedule.report.ownerInstallationId!==previewOwner || item.schedule.report.reportRevision!==revision || item.state!=="overdue") throw new Error("Schedule changed or belongs to another computer");
      item.state="undo_pending";item.deadline=Date.now()+10_000;item.schedule.report.state="undo_pending";item.schedule.report.reportRevision++; write(store);
    },
    async senderIdentity() { return defaultAccount(); },
    async createDraft(mode, sourceId, account) {
      const store = read();
      const existing = mode !== "new" && store.drafts.find((d) => d.mode === mode && d.sourceId === sourceId);
      if (existing) return existing;
      const d: Draft = { id: crypto.randomUUID(), revision: 0, account: account ?? defaultAccount(), mode, sourceId: sourceId ?? null, threadId: null, replyId: null, references: [], to: "", cc: "", bcc: "", subject: "", body: "", followUpTaskId: null, attachments: [], updatedAt: Date.now() };
      if (sourceId) {
        const detail = await getSourceThread(sourceId);
        // Reply/replyAll/forward always send from the thread's owning account, never the "new message" default.
        if (mode !== "new") d.account = account ?? detail.thread.accountId;
        const message = detail.messages.find((m) => m.id === sourceId)!;
        d.subject = detail.thread.subject;
        d.body = `\n\nOn ${attributionDate(message.sentAt)}, ${message.sender} wrote:\n${message.bodyText.split("\n").map((line) => `> ${line}`).join("\n")}`;
        if (mode === "forward") {
          d.subject = `Fwd: ${d.subject}`;
          const attribution = `---------- Forwarded message ----------\nFrom: ${message.sender}\nDate: ${attributionDate(message.sentAt)}\nSubject: ${detail.thread.subject}\nTo: ${message.recipients.join(", ")}\n\n`;
          if (message.bodyHtml.trim()) {
            d.body = "";
            d.forwardedContent = { html: `<div>${plainTextToHtml(attribution)}</div>${message.bodyHtml}`, text: attribution + message.bodyText };
          } else {
            d.body = `\n\n${attribution}${message.bodyText}`;
          }
        } else {
          d.to = message.sender;
          d.threadId = detail.thread.providerThreadId;
          d.replyId = `${sourceId}@threestrands.local`;
          d.references = [d.replyId];
        }
      }
      store.drafts.push(d); write(store); return d;
    },
    async setDraftAccount(id, account) {
      const store = read(); const d = draft(store, id);
      if (d.mode !== "new") throw new Error("Only new messages can change the sending account");
      d.account = account; d.revision++; d.updatedAt = Date.now(); write(store); return d;
    },
    async saveDraft(next) {
      const store = read(); const previous = draft(store, next.id);
      if (previous.revision !== next.revision) throw new Error("Draft changed elsewhere. Reopen it.");
      Object.assign(previous, next, { revision: next.revision + 1, updatedAt: Date.now() }); write(store); return previous;
    },
    async listDrafts() { return read().drafts; },
    async discardDraft(id) { const store = read(); store.drafts = store.drafts.filter((d) => d.id !== id); write(store); },
    async queueDraft(id, revision, _archiveOnSend) {
      const store = read(); const queued = store.outbox.find((o) => o.draft.id === id && o.draft.revision === revision);
      if (queued) return queued;
      const d = draft(store, id);
      if (d.revision !== revision) throw new Error("Draft is still saving");
      if (!d.to.trim() && !d.cc.trim() && !d.bcc.trim()) throw new Error("Add at least one recipient");
      if ([d.to, d.cc, d.bcc].some((v) => v.trim() && !v.includes("@"))) throw new Error("Enter a complete email address");
      if (d.attachments.some((a) => !a.ready)) throw new Error("Download or remove unavailable attachments");
      const item: OutboxItem = { id: crypto.randomUUID(), draft: d, state: "undo_pending", deadline: Date.now() + 10000, error: null };
      store.outbox.unshift(item); store.drafts = store.drafts.filter((d) => d.id !== id); write(store); return item;
    },
    async listOutbox() { const store = read(); tick(store); return store.outbox; },
    async cancelSend(id) { return cancel(id); },
    async recoverSend(id) { return cancel(id, true); },
    async reconcileSend() { throw new Error("No Gmail connection in browser preview"); },
    async attachFiles(id) {
      const files = await new Promise<File[]>((resolve) => {
        const input = document.createElement("input"); input.type = "file"; input.multiple = true; input.setAttribute("aria-label", "Choose attachments"); input.hidden = true; document.body.append(input);
        input.onchange = () => { resolve(Array.from(input.files ?? [])); input.remove(); };
        input.oncancel = () => { resolve([]); input.remove(); };
        input.click();
      });
      const store = read(); const d = draft(store, id);
      if (files.reduce((sum, f) => sum + f.size, d.attachments.reduce((sum, a) => sum + a.size, 0)) > 18 * 1024 * 1024) throw new Error("Attachments exceed the 18 MB local limit");
      d.attachments.push(...files.map((f) => ({ id: crypto.randomUUID(), name: f.name, mime: f.type || "application/octet-stream", size: f.size, ready: true, messageId: null, providerId: null, inline: false, contentId: null })));
      d.revision++; write(store); return d;
    },
    async attachInlineImage(id, name, mime, data) {
      const store = read(); const d = draft(store, id);
      if (!/^image\/(?:avif|gif|jpeg|png|webp)$/i.test(mime)) throw new Error("Paste a supported image format");
      const size = Math.floor(data.length * 3 / 4);
      if (size + d.attachments.reduce((sum, attachment) => sum + attachment.size, 0) > 18 * 1024 * 1024) throw new Error("Attachments exceed the 18 MB local limit");
      const attachmentId = crypto.randomUUID();
      const contentId = `${attachmentId}@threestrands.local`;
      d.attachments.push({ id: attachmentId, name: name || "pasted-image", mime, size, ready: true, messageId: null, providerId: null, inline: true, contentId });
      inlineImages.set(attachmentId, `data:${mime};base64,${data}`);
      d.revision++; write(store); return d;
    },
    async readInlineImage(id, attachmentId) {
      const store = read();
      const owner = store.drafts.find((candidate) => candidate.id === id)
        ?? store.outbox.find((candidate) => candidate.draft.id === id && candidate.state !== "canceled")?.draft;
      const attachment = owner?.attachments.find((candidate) => candidate.id === attachmentId && candidate.inline);
      const data = attachment && inlineImages.get(attachmentId);
      if (!data) throw new Error("Pasted image data is unavailable");
      return data;
    },
    async removeAttachment(id, attachmentId) { const store = read(); const d = draft(store, id); d.attachments = d.attachments.filter((a) => a.id !== attachmentId); inlineImages.delete(attachmentId); d.revision++; write(store); return d; },
    async fetchAttachment() { throw new Error("Attachments are simulated in browser preview"); },
  };
}
