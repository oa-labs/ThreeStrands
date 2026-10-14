import { useEffect, useState } from "react";
import { AttachmentIcon } from "./AttachmentIcon";
import { mailClient } from "./data/client";
import type { MessageAttachment } from "./domain";

/**
 * An image attachment's picture, or its kind icon until the picture loads and
 * whenever it cannot. The native side validates the bytes as a supported
 * raster before returning a data URI, so nothing here fetches a remote
 * resource or renders a scriptable format. Decorative: the filename names it.
 */
export function AttachmentThumbnail({ messageId, attachment, enabled }: {
  messageId: string;
  attachment: MessageAttachment;
  enabled: boolean;
}) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    setSrc(null);
    if (!enabled) return;
    let active = true;
    void mailClient.fetchAttachmentImage(messageId, attachment.id).then((value) => {
      if (active && value.startsWith("data:image/")) setSrc(value);
    }).catch(() => undefined);
    return () => { active = false; };
  }, [attachment.id, enabled, messageId]);
  if (!src) return <AttachmentIcon filename={attachment.filename} mimeType={attachment.mimeType} size="sm" />;
  return <img className="attachment-thumbnail" src={src} alt="" aria-hidden="true" draggable={false} onError={() => setSrc(null)} />;
}
