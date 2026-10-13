import { useEffect, useState } from "react";
import { Star } from "lucide-react";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

export const FILES_CHANGED = "threestrands:files-changed";
const reads = new Map<string, Promise<[string, string][]>>();
function readStars(messageId: string) {
  let read = reads.get(messageId);
  if (!read) {
    read = Promise.resolve().then(() => mailClient.fileStars([messageId]));
    reads.set(messageId, read);
    if (reads.size>200) reads.delete(reads.keys().next().value!);
    void read.catch(() => reads.delete(messageId));
  }
  return read;
}

export function FileStarButton({ messageId, attachmentId, filename, threadId, accountId, starred: initial }: {
  messageId: string; attachmentId: string; filename: string; threadId: string; accountId?: string; starred?: boolean;
}) {
  const [starred, setStarred] = useState(initial ?? false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    const load = () => { void readStars(messageId).then((stars) => {
      if (active) { setStarred(stars.some(([message, attachment]) => message === messageId && attachment === attachmentId)); setError(null); }
    }).catch((reason) => { if (active) setError(errorMessage(reason)); }); };
    if (initial === undefined) load(); else setStarred(initial);
    const changed = () => load();
    window.addEventListener(FILES_CHANGED, changed);
    return () => { active = false; window.removeEventListener(FILES_CHANGED, changed); };
  }, [attachmentId, initial, messageId]);
  const toggle = async () => {
    const previous = starred;
    setPending(true); setError(null); setStarred(!previous);
    try {
      const owner = accountId ?? (await mailClient.getThread(threadId)).thread.accountId;
      await mailClient.setFileStar(owner, messageId, attachmentId, !previous);
      reads.delete(messageId);
      window.dispatchEvent(new Event(FILES_CHANGED));
    } catch (reason) { setStarred(previous); setError(errorMessage(reason)); }
    finally { setPending(false); }
  };
  return <span className="file-star-control">
    <button type="button" className="btn-icon btn-icon-sm" aria-label={`${starred ? "Unstar" : "Star"} ${filename}`} aria-pressed={starred}
      title="Important for later · stored on this device" disabled={pending} onClick={() => void toggle()}>
      <Star size={ICON_SIZE.sm} fill={starred ? "currentColor" : "none"} />
    </button>
    {error ? <span className="form-error" role="alert">{error}<button type="button" className="btn-link" onClick={() => { reads.delete(messageId); setError(null); void readStars(messageId).then((stars) => setStarred(stars.some(([m,a]) => m===messageId && a===attachmentId))).catch((reason) => setError(errorMessage(reason))); }}>Retry</button></span> : null}
  </span>;
}
