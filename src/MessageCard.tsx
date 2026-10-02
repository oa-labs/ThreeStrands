import { Check, ChevronDown, ChevronUp, Copy, Download, ExternalLink, Forward, Paperclip, Reply, ReplyAll } from "lucide-react";
import { memo, useCallback, useMemo, useState, type KeyboardEvent, type MouseEvent } from "react";
import { HoverTooltip } from "./AppChrome";
import { CalendarAttachmentGroup, isCalendarAttachment } from "./CalendarAttachment";
import type { OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account, Message } from "./domain";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import { errorMessage } from "./errors";
import { isInlineImageAttachment, normalizeContentId, referencedImageContentIds } from "./inlineAttachments";
import { decodeHtmlEntities, SafeMessage } from "./SafeMessage";
import type { FontFamily } from "./settings";
import { formatAttachmentSize, formatMailTimestamp, splitAttachmentName } from "./threadPresentation";

export type MessageResponseKind = "reply" | "replyAll" | "forward";

type MessageCardProps = {
  message: Message;
  index: number;
  isLatest: boolean;
  isExpanded: boolean;
  isActive: boolean;
  accounts: Account[];
  /** Set when this card shows a reply still in the outbox. */
  queuedItem: OutboxItem | undefined;
  loadRemoteImages: boolean;
  theme: "light" | "dark";
  fontScale: number;
  fontFamily: FontFamily;
  emailMinimumFontSize?: number;
  onActivate: (messageId: string) => void;
  onToggle: (messageId: string, isExpanded: boolean) => void;
  onRespond: (kind: MessageResponseKind, messageId: string) => void;
  onRegisterNode: (messageId: string, isLatest: boolean, node: HTMLElement | null) => void;
  onImageClick: (src: string) => void;
  onNotice: (message: string) => void;
};

/**
 * One message in the reader. Memoized, and fed stable callbacks, so App
 * renders unrelated to this message (search keystrokes, notices, sync
 * status) skip re-parsing its HTML and re-rendering its email frame.
 */
export const MessageCard = memo(function MessageCard({
  message,
  index,
  isLatest,
  isExpanded,
  isActive,
  accounts,
  queuedItem,
  loadRemoteImages,
  theme,
  fontScale,
  fontFamily,
  emailMinimumFontSize = 0,
  onActivate,
  onToggle,
  onRespond,
  onRegisterNode,
  onImageClick,
  onNotice,
}: MessageCardProps) {
  const parsedSender = parseAddress(message.sender);
  const senderAccount = accounts.find(
    (account) => account.email.toLocaleLowerCase() === parsedSender.email.toLocaleLowerCase(),
  );
  const senderName = senderAccount?.displayName?.trim() || parsedSender.name;
  const senderDisplayName = formatDisplayName(senderName);
  const downloadableAttachments = useMemo(() => {
    const referencedContentIds = referencedImageContentIds(message.bodyHtml);
    return message.attachments.filter((attachment) => !isInlineImageAttachment(attachment, referencedContentIds));
  }, [message.attachments, message.bodyHtml]);
  const cardBodyId = `message-body-${index}`;
  const messageId = message.id;

  const activateMessage = useCallback(() => onActivate(messageId), [messageId, onActivate]);
  const toggleMessage = useCallback(() => onToggle(messageId, isExpanded), [isExpanded, messageId, onToggle]);
  const toggleOnEnter = useCallback((event: KeyboardEvent) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    toggleMessage();
  }, [toggleMessage]);
  const registerNode = useCallback(
    (node: HTMLElement | null) => onRegisterNode(messageId, isLatest, node),
    [isLatest, messageId, onRegisterNode],
  );
  const resolveImage = useCallback((url: string) => {
    if (!/^cid:/i.test(url)) return mailClient.fetchRemoteImage(url);
    const contentId = normalizeContentId(url.slice(4));
    const embedded = message.attachments.find((attachment) =>
      attachment.contentId
      && normalizeContentId(attachment.contentId) === contentId
    );
    if (!embedded) return Promise.reject(new Error("Embedded image not found"));
    return queuedItem
      ? mailClient.readInlineImage(queuedItem.draft.id, embedded.id)
      : mailClient.fetchAttachmentImage(message.id, embedded.id);
  }, [message.attachments, message.id, queuedItem]);

  if (!isExpanded) {
    return (
      <article
        className={`message message-card message-card-collapsed ${isActive ? "message-active" : ""}`}
        ref={registerNode}
        data-message-id={message.id}
        onFocusCapture={activateMessage}
        onMouseDown={activateMessage}
      >
        <header className="message-card-header">
          <button
            type="button"
            className="message-card-toggle"
            aria-expanded={false}
            aria-controls={cardBodyId}
            onClick={toggleMessage}
            onKeyDown={toggleOnEnter}
          >
            <span className="message-card-sender">{senderDisplayName}</span>
            <CollapsedSnippet bodyText={message.bodyText} />
            {downloadableAttachments.length > 0 ? <Paperclip size={13} aria-label="Has attachments" /> : null}
            <time>{formatMailTimestamp(message.sentAt)}</time>
            <ChevronDown size={14} className="message-card-chevron" />
          </button>
        </header>
        <div id={cardBodyId} hidden />
      </article>
    );
  }

  const recipients = splitAddressList(message.recipients.join(", "));
  const headerDetails = (
    <div className="message-header-details">
      <div className="message-sender-row">
        <strong><AddressWithCopy address={message.sender} displayName={senderDisplayName} /></strong>
        {queuedItem ? null : (
          <div className="message-header-actions">
            <HoverTooltip label="Reply" placement="bottom">
              <button
                type="button"
                className="message-header-action"
                aria-label="Reply"
                onClick={() => onRespond("reply", message.id)}
              >
                <Reply size={14} />
              </button>
            </HoverTooltip>
            <HoverTooltip label="Reply all" placement="bottom">
              <button
                type="button"
                className="message-header-action"
                aria-label="Reply All"
                onClick={() => onRespond("replyAll", message.id)}
              >
                <ReplyAll size={14} />
              </button>
            </HoverTooltip>
            <HoverTooltip label="Forward" placement="bottom">
              <button
                type="button"
                className="message-header-action"
                aria-label="Forward"
                onClick={() => onRespond("forward", message.id)}
              >
                <Forward size={14} />
              </button>
            </HoverTooltip>
          </div>
        )}
        <time>{formatMailTimestamp(message.sentAt)}</time>
      </div>
      <div className="message-recipients">
        to{" "}
        {recipients.map((recipient, recipientIndex) => (
          <span key={recipient}>
            {recipientListSeparator(recipientIndex, recipients.length)}
            <AddressWithCopy
              address={recipient}
              displayName={formatDisplayName(parseAddress(recipient).name)}
            />
          </span>
        ))}
      </div>
    </div>
  );
  return (
    <article
      className={`message message-card message-card-expanded ${isActive ? "message-active" : ""}`}
      ref={registerNode}
      data-message-id={message.id}
      onFocusCapture={activateMessage}
      onMouseDown={activateMessage}
    >
      <header
        className="message-expanded-header"
      >
        {headerDetails}
        <button
          type="button"
          className="message-expanded-toggle"
          aria-expanded={true}
          aria-controls={cardBodyId}
          aria-label={`Collapse message from ${senderDisplayName}, ${formatMailTimestamp(message.sentAt)}`}
          onClick={toggleMessage}
          onKeyDown={toggleOnEnter}
        >
          <ChevronUp size={14} />
        </button>
      </header>
      <div id={cardBodyId} className="message-card-body">
        <SafeMessage
          html={message.bodyHtml}
          text={message.bodyText}
          loadImages={loadRemoteImages}
          imageCacheKey={message.id}
          onImageClick={onImageClick}
          onEnterKey={toggleMessage}
          resolveImage={resolveImage}
          theme={theme}
          fontScale={fontScale}
          fontFamily={fontFamily}
          emailMinimumFontSize={emailMinimumFontSize}
          tone={isLatest ? "current" : message.unread ? "default" : "muted"}
        />
        {downloadableAttachments.length > 0 ? (
          <div className="message-attachments" aria-label="Attachments">
            {downloadableAttachments.some(isCalendarAttachment) ? (
              <CalendarAttachmentGroup
                messageId={message.id}
                attachments={downloadableAttachments.filter(isCalendarAttachment)}
                onError={onNotice}
              />
            ) : null}
            {downloadableAttachments.filter((attachment) => !isCalendarAttachment(attachment)).map((attachment) => {
                const attachmentName = splitAttachmentName(attachment.filename);
                return (
                <div className="message-attachment" key={attachment.id}>
                  <HoverTooltip title={`Download ${attachment.filename}`} placement="bottom"><button
                    type="button"
                    className="attachment-badge"
                    aria-label={`View ${attachment.filename}`}
                    onClick={() => {
                      void mailClient.openAttachment(message.id, attachment.id).catch((reason: unknown) => {
                        onNotice(`Could not open attachment: ${errorMessage(reason)}`);
                      });
                    }}
                  >
                    <Paperclip size={14} />
                    <span className="attachment-name">
                      <span className="attachment-name-base">{attachmentName.base}</span>
                      {attachmentName.extension ? <span className="attachment-name-ext">{attachmentName.extension}</span> : null}
                    </span>
                    <small>{formatAttachmentSize(attachment.size)}</small>
                    <ExternalLink size={13} />
                  </button></HoverTooltip>
                  <button
                    type="button"
                    className="attachment-download"
                    aria-label={`Download ${attachment.filename}`}
                    onClick={() => {
                      void mailClient.saveAttachment(message.id, attachment.id).catch((reason: unknown) => {
                        onNotice(`Could not download attachment: ${errorMessage(reason)}`);
                      });
                    }}
                  >
                    <Download size={14} />
                  </button>
                </div>
                );
              })}
          </div>
        ) : null}
      </div>
    </article>
  );
});

function CollapsedSnippet({ bodyText }: { bodyText: string }) {
  const snippet = useMemo(() => messageSnippet(bodyText), [bodyText]);
  return <span className="message-card-snippet">{snippet}</span>;
}

function AddressWithCopy({ address, displayName }: { address: string; displayName?: string }) {
  const [copied, setCopied] = useState(false);
  const parsedAddress = parseAddress(address);
  const parsed = displayName ? { ...parsedAddress, name: displayName } : parsedAddress;

  const handleCopy = async (event: MouseEvent) => {
    event.stopPropagation();
    await navigator.clipboard.writeText(parsed.email);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <span className="address" tabIndex={0} onClick={(event) => event.stopPropagation()}>
      <span className="address-name">{parsed.name}</span>
      <span className="address-popover">
        <span className="address-email">{parsed.email}</span>
        <button
          type="button"
          className="address-copy"
          aria-label={copied ? "Copied" : `Copy ${parsed.email}`}
          onClick={handleCopy}
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
        </button>
      </span>
    </span>
  );
}

function messageSnippet(bodyText: string, maxLength = 140): string {
  const collapsed = decodeHtmlEntities(bodyText).replace(/\s+/g, " ").trim();
  return collapsed.length > maxLength ? `${collapsed.slice(0, maxLength).trimEnd()}…` : collapsed;
}

function recipientListSeparator(index: number, recipientCount: number): string {
  if (index === 0) return "";
  if (index === recipientCount - 1) return recipientCount === 2 ? " and " : ", and ";
  return ", ";
}
