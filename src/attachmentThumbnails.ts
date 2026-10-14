import type { MessageAttachment } from "./domain";
import { EMAIL_IMAGE_LIMITS } from "./emailRenderingPolicy";

/** Raster types the native image validator accepts; SVG is excluded because it can carry script and remote references. */
const THUMBNAIL_MIME_TYPES = new Set(["image/avif", "image/gif", "image/jpeg", "image/png", "image/webp"]);

/** A thumbnail is held as a base64 data URI, so it counts against the message budget at its encoded size. */
function encodedSize(bytes: number): number {
  return Math.ceil(bytes / 3) * 4;
}

function thumbnailMimeType(mimeType: string): boolean {
  return THUMBNAIL_MIME_TYPES.has(mimeType.split(";")[0].trim().toLowerCase());
}

/**
 * The attachments whose chips show a picture of the file instead of its kind
 * icon, in message order: supported rasters within the per-file cap, until the
 * per-message count or data-URI budget runs out. Everything else keeps its icon.
 */
export function thumbnailAttachmentIds(attachments: readonly MessageAttachment[]): Set<string> {
  const selected = new Set<string>();
  let bytes = 0;
  for (const attachment of attachments) {
    if (selected.size >= EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailsPerMessage) break;
    if (!thumbnailMimeType(attachment.mimeType)) continue;
    if (!Number.isFinite(attachment.size) || attachment.size <= 0) continue;
    if (attachment.size > EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailBytes) continue;
    const encoded = encodedSize(attachment.size);
    if (bytes + encoded > EMAIL_IMAGE_LIMITS.maxDataUriBytesPerMessage) continue;
    bytes += encoded;
    selected.add(attachment.id);
  }
  return selected;
}
