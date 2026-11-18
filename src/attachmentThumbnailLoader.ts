import { mailClient } from "./data/client";
import { EMAIL_IMAGE_LIMITS, parseAttachmentThumbnailDataUri } from "./emailRenderingPolicy";

const RASTER = /^data:image\/(?:png|jpeg|gif|webp|avif);base64,/i;

/** Browser-only fallback. Only the tiny canvas result survives the request. */
export async function resizeAttachmentThumbnail(source: string): Promise<string> {
  if (!RASTER.test(source)) throw new Error("Unsupported attachment image");
  const image = new Image();
  const canvas = document.createElement("canvas");
  try {
    await new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error("Attachment image could not be decoded"));
      image.src = source;
    });
    if (!(image.naturalWidth > 0 && image.naturalHeight > 0)) throw new Error("Empty attachment image");
    const ratio = Math.min(1, EMAIL_IMAGE_LIMITS.attachmentThumbnailEdgePx / Math.max(image.naturalWidth, image.naturalHeight));
    canvas.width = Math.max(1, Math.round(image.naturalWidth * ratio));
    canvas.height = Math.max(1, Math.round(image.naturalHeight * ratio));
    const context = canvas.getContext("2d");
    if (!context) throw new Error("Attachment thumbnail unavailable");
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/png");
  } finally {
    image.onload = null; image.onerror = null; image.src = "";
    canvas.width = 0; canvas.height = 0;
  }
}

// Keep both native work and fallback decodes bounded across all open messages.
let active = 0;
const queue: (() => void)[] = [];
const pending = new Map<string, Promise<string | null>>();
function run<T>(job: () => Promise<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    const start = () => {
      active++;
      void job().then(resolve, reject).finally(() => { active--; queue.shift()?.(); });
    };
    if (active < EMAIL_IMAGE_LIMITS.maxConcurrentAttachmentThumbnails) start();
    else if (queue.length < EMAIL_IMAGE_LIMITS.maxPendingGlobally) queue.push(start);
    else reject(new Error("Thumbnail queue is full"));
  });
}

export function loadAttachmentThumbnail(messageId: string, attachmentId: string): Promise<string | null> {
  const key = JSON.stringify([messageId, attachmentId]);
  const shared = pending.get(key);
  if (shared) return shared;
  const request = run(async () => {
    const result = await mailClient.fetchAttachmentThumbnail(messageId, attachmentId);
    if (!result.dataUri) return null;
    const src = result.needsResize ? await resizeAttachmentThumbnail(result.dataUri) : result.dataUri;
    if (!parseAttachmentThumbnailDataUri(src)) throw new Error("Invalid attachment thumbnail");
    return src;
  });
  pending.set(key, request);
  void request.finally(() => { if (pending.get(key) === request) pending.delete(key); }).catch(() => {});
  return request;
}
