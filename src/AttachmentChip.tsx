import { Download, ExternalLink, TriangleAlert } from "lucide-react";
import { useId, type ReactNode } from "react";
import { HoverTooltip } from "./AppChrome";
import { attachmentRisk } from "./attachmentRisk";
import type { MessageAttachment } from "./domain";
import { FileStarButton } from "./FileStarButton";
import { ICON_SIZE } from "./iconSizes";
import { formatAttachmentSize, splitAttachmentName } from "./threadPresentation";

/**
 * One reader attachment: the filename opens the file in its default app, the
 * save segment writes a copy through a Save dialog, and the trailing star
 * keeps it in Files. Each target names its own verb in a tooltip, and the
 * chip stretches to its grid column so a column of chips shares a right edge.
 * A file type that can harm the computer carries a warning glyph and says why.
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
  const risk = attachmentRisk(attachment.filename, attachment.mimeType);
  const riskId = useId();
  const openLabel = `Open ${attachment.filename}`;
  const saveLabel = `Save ${attachment.filename} as…`;
  return (
    <div className={`message-attachment${risk ? " message-attachment-risky" : ""}`}>
      <HoverTooltip label={risk ? `${openLabel} · ${risk.reason}` : openLabel} placement="bottom">
        <button type="button" className="attachment-badge" aria-label={openLabel} aria-describedby={risk ? riskId : undefined} onClick={onOpen}>
          {icon}
          <span className="attachment-name">
            <span className="attachment-name-base">{base}</span>
            {extension ? <span className="attachment-name-ext">{extension}</span> : null}
          </span>
          {risk ? <TriangleAlert className="attachment-risk" size={ICON_SIZE.xs} aria-hidden="true" /> : null}
          {risk ? <span id={riskId} hidden>{risk.reason}</span> : null}
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
