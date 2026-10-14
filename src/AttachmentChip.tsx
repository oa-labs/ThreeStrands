import { Download, ExternalLink } from "lucide-react";
import type { ReactNode } from "react";
import { HoverTooltip } from "./AppChrome";
import type { MessageAttachment } from "./domain";
import { FileStarButton } from "./FileStarButton";
import { ICON_SIZE } from "./iconSizes";
import { formatAttachmentSize, splitAttachmentName } from "./threadPresentation";

/**
 * One reader attachment: the filename opens the file in its default app, the
 * save segment writes a copy through a Save dialog, and the trailing star
 * keeps it in Files. Each target names its own verb in a tooltip, and the
 * chip stretches to its grid column so a column of chips shares a right edge.
 */
export function AttachmentChip({ messageId, attachment, icon, starrable, onOpen, onSave }: {
  messageId: string;
  attachment: MessageAttachment;
  icon: ReactNode;
  starrable: boolean;
  onOpen(): void;
  onSave(): void;
}) {
  const { base, extension } = splitAttachmentName(attachment.filename);
  const openLabel = `Open ${attachment.filename}`;
  const saveLabel = `Save ${attachment.filename} as…`;
  return (
    <div className="message-attachment">
      <HoverTooltip label={openLabel} placement="bottom">
        <button type="button" className="attachment-badge" aria-label={openLabel} onClick={onOpen}>
          {icon}
          <span className="attachment-name">
            <span className="attachment-name-base">{base}</span>
            {extension ? <span className="attachment-name-ext">{extension}</span> : null}
          </span>
          <small>{formatAttachmentSize(attachment.size)}</small>
          <ExternalLink size={ICON_SIZE.xs} aria-hidden="true" />
        </button>
      </HoverTooltip>
      <HoverTooltip label={saveLabel} placement="bottom">
        <button type="button" className="attachment-download" aria-label={saveLabel} onClick={onSave}>
          <Download size={ICON_SIZE.sm} aria-hidden="true" />
        </button>
      </HoverTooltip>
      {starrable ? <FileStarButton messageId={messageId} attachmentId={attachment.id} filename={attachment.filename} /> : null}
    </div>
  );
}
