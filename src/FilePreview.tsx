import { useEffect, useState } from "react";
import { attachmentRisk } from "./attachmentRisk";
import { mailClient } from "./data/client";
import type { WorkspaceFile } from "./domain";
import { FILE_PREVIEW_LIMITS } from "./emailRenderingPolicy";

const PNG_DATA_URI = "data:image/png;base64,";
/** Recent previews, newest last, so moving back to a file shows it at once. */
const cache = new Map<string, string | null>();

function remember(key: string, preview: string | null) {
  cache.delete(key);
  cache.set(key, preview);
  while (cache.size > FILE_PREVIEW_LIMITS.maxCachedPreviews) cache.delete(cache.keys().next().value!);
}

/**
 * Whether to ask for a preview at all. Risky types never get one, files over
 * the native size cap would only be refused after downloading, and a file
 * that isn't on this device can't be fetched while offline.
 */
export function wantsFilePreview(file: WorkspaceFile, online: boolean): boolean {
  if (attachmentRisk(file.attachment.filename, file.attachment.mimeType)) return false;
  if (!(file.attachment.size > 0) || file.attachment.size > FILE_PREVIEW_LIMITS.maxSourceBytes) return false;
  return online || file.availability === "offline";
}

/**
 * A picture of the selected file, drawn natively by Quick Look on macOS.
 * Nothing shows until one arrives, and nothing at all when the file has none,
 * so the details never jump for files without previews. Clicking it opens
 * the file like the Open button, which stays the accessible way to do so.
 */
export function FilePreview({ file, online, onOpen }: { file: WorkspaceFile; online: boolean; onOpen(): void }) {
  const key = JSON.stringify([file.messageId, file.attachment.id]);
  const wanted = wantsFilePreview(file, online);
  const [preview, setPreview] = useState<{ key: string; src: string | null } | null>(null);
  useEffect(() => {
    if (!wanted) return;
    if (cache.has(key)) { setPreview({ key, src: cache.get(key) ?? null }); return; }
    let active = true;
    void mailClient.filePreview(file.messageId, file.attachment.id).then((value) => {
      const src = typeof value === "string" && value.startsWith(PNG_DATA_URI) ? value : null;
      remember(key, src);
      if (active) setPreview({ key, src });
    }).catch(() => undefined);
    return () => { active = false; };
  }, [file.attachment.id, file.messageId, key, wanted]);
  const src = wanted && preview?.key === key ? preview.src : null;
  if (!src) return null;
  return <div className="file-detail-preview" aria-hidden="true" onClick={onOpen}>
    <img src={src} alt="" draggable={false} onError={() => setPreview({ key, src: null })} />
  </div>;
}

/** Test seam: previews persist across renders by design. */
export function clearFilePreviewCache() {
  cache.clear();
}
