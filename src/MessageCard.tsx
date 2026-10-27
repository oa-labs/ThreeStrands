import { AttachmentChip } from "./AttachmentChip";
import { AttachmentThumbnail } from "./AttachmentThumbnail";
import { thumbnailAttachmentIds } from "./attachmentThumbnails";
import { Check, ChevronDown, ChevronUp, Copy, CopyPlus, Forward, Paperclip, Reply, ReplyAll } from "lucide-react";
import { Fragment, memo, useCallback, useContext, useMemo, useState, type KeyboardEvent, type MouseEvent } from "react";
import { HoverTooltip } from "./AppChrome";
import { CalendarAttachmentGroup, isCalendarAttachment } from "./CalendarAttachment";
import type { InvitationActions } from "./InvitationSection";
import { AddressWithCard } from "./AddressWithCard";
import { ContactCardContext } from "./ContactCard";
import type { OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account, Message } from "./domain";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import { errorMessage } from "./errors";
import { isInlineImageAttachment, normalizeContentId, referencedImageContentIds } from "./inlineAttachments";
import { decodeHtmlEntities, SafeMessage, type LinkReveal } from "./SafeMessage";
import type { ThreadTextIndex } from "./quotedHistory";
import type { EmailTextColors } from "./emailRenderingPolicy";
import type { FontFamily } from "./settings";
import { formatMailTimestamp } from "./threadPresentation";
import { ICON_SIZE } from "./iconSizes";
import { selectedMessageQuote } from "./selectedMessageQuote";
import { formatShortcutStep } from "./shortcutCatalog";
import { useTrustReport } from "./SenderSections";
import type { TrustLink } from "./senderSignals";
import { TrustBadge, TrustBanner } from "./TrustBadge";

/** A long recipient list (To and Cc together) shows this many names, then "and N more" to reveal the rest. */
const RECIPIENT_PREVIEW_LIMIT = 20;

export type MessageResponseKind = "reply" | "replyAll" | "forward";

type MessageCardProps = {
  message: Message;
  invitationActions?: InvitationActions;
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
  emailZoom?: number;
  emailMinimumFontSize?: number;
  emailTextColors?: EmailTextColors;
  /** The conversation's text index; this card folds text repeated from messages before `index`. */
  threadText?: ThreadTextIndex;
  onActivate: (messageId: string) => void;
  onToggle: (messageId: string, isExpanded: boolean) => void;
  onRespond: (kind: MessageResponseKind, messageId: string) => void;
  /** Starts an unaddressed new message from this one; offered only on messages an account sent. */
  onEditAsNew?: (messageId: string) => void;
  onRegisterNode: (messageId: string, isLatest: boolean, node: HTMLElement | null) => void;
  onImageClick: (src: string) => void;
  onNotice: (message: string) => void;
  /** A link in this message to show, such as one the Trust section warns about. */
  revealLink?: LinkReveal | null;
  /**
   * The conversation's subject. When set, a message someone else sent shows
   * its trust verdict beside the sender and its strongest warnings above the body.
   */
  subject?: string;
  /** Whether the conversation is bulk mail, as the Trust section judges it. */
  bulk?: boolean;
  /** Shows a link a trust warning is about. */
  onShowLink?: (messageId: string, link: TrustLink) => void;
};

/**
 * One message in the reader. Memoized, and fed stable callbacks, so App
 * renders unrelated to this message (search keystrokes, notices, sync
 * status) skip re-parsing its HTML and re-rendering its email frame.
 */
export const MessageCard = memo(function MessageCard({
  message,
  invitationActions,
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
  emailZoom,
  emailMinimumFontSize = 0,
  emailTextColors,
  threadText,
  onActivate,
  onToggle,
  onRespond,
  onEditAsNew,
  onRegisterNode,
  onImageClick,
  onNotice,
  revealLink = null,
  subject,
  bulk = false,
  onShowLink,
}: MessageCardProps) {
  const parsedSender = parseAddress(message.sender);
  const senderAccount = accounts.find(
    (account) => account.email.toLocaleLowerCase() === parsedSender.email.toLocaleLowerCase(),
  );
  const senderName = senderAccount?.displayName?.trim() || parsedSender.name;
  const priorThreadText = useMemo(() => threadText?.before(index), [threadText, index]);
  const trust = useTrustReport({
    message,
    subject: subject ?? "",
    bulk,
    prior: priorThreadText,
    enabled: isExpanded && subject !== undefined && !senderAccount && !queuedItem,
  });
  const showTrustLink = useMemo(() => onShowLink ? (link: TrustLink) => onShowLink(message.id, link) : undefined, [onShowLink, message.id]);
  const senderDisplayName = formatDisplayName(senderName);
  const downloadableAttachments = useMemo(() => {
    const referencedContentIds = referencedImageContentIds(message.bodyHtml);
    return message.attachments.filter((attachment) => !isInlineImageAttachment(attachment, referencedContentIds));
  }, [message.attachments, message.bodyHtml]);
  const thumbnailIds = useMemo(() => thumbnailAttachmentIds(downloadableAttachments), [downloadableAttachments]);
  const cardBodyId = `message-body-${index}`;
  const messageId = message.id;
  const preserveSelectedQuote = (event: MouseEvent<HTMLButtonElement>) => {
    // Read iframe selections before the button's default mouse focus blurs them.
    if (event.button === 0 && selectedMessageQuote(messageId)) event.preventDefault();
  };

  const [showAllRecipients, setShowAllRecipients] = useState(false);
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
            {downloadableAttachments.length > 0 ? <Paperclip size={ICON_SIZE.xs} aria-label="Has attachments" /> : null}
            <time>{formatMailTimestamp(message.sentAt)}</time>
            <ChevronDown size={ICON_SIZE.sm} className="message-card-chevron" />
          </button>
        </header>
        <div id={cardBodyId} hidden />
      </article>
    );
  }

  const ownEmails = new Set(accounts.map((account) => account.email.toLocaleLowerCase()));
  const recipientGroups = visibleRecipientGroups(message, showAllRecipients ? Infinity : RECIPIENT_PREVIEW_LIMIT);
  const hiddenRecipientCount = recipientGroups.hiddenCount;
  const headerDetails = (
    <div className="message-header-details">
      <div className="message-sender-row">
        <strong><MessageAddress address={message.sender} displayName={senderDisplayName} own={ownEmails} /></strong>
        {trust ? <TrustBadge report={trust} onShowLink={showTrustLink} /> : null}
        {queuedItem ? null : (
          <div className="message-header-actions">
            <HoverTooltip label="Reply" placement="bottom">
              <button
                type="button"
                className="btn-icon btn-icon-sm"
                aria-label="Reply"
                onMouseDown={preserveSelectedQuote}
                onClick={() => onRespond("reply", message.id)}
              >
                <Reply size={ICON_SIZE.sm} />
              </button>
            </HoverTooltip>
            <HoverTooltip label="Reply all" placement="bottom">
              <button
                type="button"
                className="btn-icon btn-icon-sm"
                aria-label="Reply All"
                onMouseDown={preserveSelectedQuote}
                onClick={() => onRespond("replyAll", message.id)}
              >
                <ReplyAll size={ICON_SIZE.sm} />
              </button>
            </HoverTooltip>
            <HoverTooltip label="Forward" placement="bottom">
              <button
                type="button"
                className="btn-icon btn-icon-sm"
                aria-label="Forward"
                onMouseDown={preserveSelectedQuote}
                onClick={() => onRespond("forward", message.id)}
              >
                <Forward size={ICON_SIZE.sm} />
              </button>
            </HoverTooltip>
            {senderAccount && onEditAsNew ? (
              <HoverTooltip label="Edit as new" shortcut={formatShortcutStep("Shift+c")} placement="bottom">
                <button
                  type="button"
                  className="btn-icon btn-icon-sm"
                  aria-label="Edit as New"
                  onClick={() => onEditAsNew(message.id)}
                >
                  <CopyPlus size={ICON_SIZE.sm} />
                </button>
              </HoverTooltip>
            ) : null}
          </div>
        )}
        <time>{formatMailTimestamp(message.sentAt)}</time>
      </div>
      {recipientGroups.groups.length > 0 ? <div className="message-recipients">
        {recipientGroups.groups.map((group, groupIndex) => {
          const isLastGroup = groupIndex === recipientGroups.groups.length - 1;
          const listLength = group.addresses.length + (isLastGroup && hiddenRecipientCount ? 1 : 0);
          return (
            <Fragment key={group.label}>
              {groupIndex > 0 ? ", " : null}
              {group.label}{" "}
              {group.addresses.map((recipient, recipientIndex) => (
                <span key={recipient}>
                  {recipientListSeparator(recipientIndex, listLength)}
                  <MessageAddress
                    address={recipient}
                    displayName={ownEmails.has(parseAddress(recipient).email.toLocaleLowerCase())
                      ? "me"
                      : formatDisplayName(parseAddress(recipient).name)}
                    own={ownEmails}
                  />
                </span>
              ))}
            </Fragment>
          );
        })}
        {hiddenRecipientCount ? <>
          {", and "}
          <button type="button" className="btn-link message-recipients-more" onClick={() => setShowAllRecipients(true)}>
            {hiddenRecipientCount} more
          </button>
        </> : null}
      </div> : null}
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
          className="btn-icon btn-icon-sm message-expanded-toggle"
          aria-expanded={true}
          aria-controls={cardBodyId}
          aria-label={`Collapse message from ${senderDisplayName}, ${formatMailTimestamp(message.sentAt)}`}
          onClick={toggleMessage}
          onKeyDown={toggleOnEnter}
        >
          <ChevronUp size={ICON_SIZE.sm} />
        </button>
      </header>
      <div id={cardBodyId} className="message-card-body">
        {trust ? <TrustBanner messageId={message.id} report={trust} onShowLink={showTrustLink} /> : null}
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
          emailZoom={emailZoom}
          emailMinimumFontSize={emailMinimumFontSize}
          emailTextColors={emailTextColors}
          tone={isLatest ? "current" : message.unread ? "default" : "muted"}
          priorThreadText={priorThreadText}
          revealLink={revealLink}
        />
        {downloadableAttachments.length > 0 ? (
          <div className="message-attachments" aria-label="Attachments">
            {downloadableAttachments.some(isCalendarAttachment) ? (
              <CalendarAttachmentGroup
                threadId={message.threadId}
                messageId={message.id}
                attachments={downloadableAttachments.filter(isCalendarAttachment)}
                onError={onNotice}
                invitationActions={invitationActions}
              />
            ) : null}
            {downloadableAttachments.filter((attachment) => !isCalendarAttachment(attachment)).map((attachment) => (
              <AttachmentChip
                key={attachment.id}
                messageId={message.id}
                attachment={attachment}
                icon={<AttachmentThumbnail messageId={message.id} attachment={attachment} enabled={!queuedItem && thumbnailIds.has(attachment.id)} />}
                starrable
                onOpen={() => {
                  void mailClient.openAttachment(message.id, attachment.id).catch((reason: unknown) => {
                    onNotice(`Could not open attachment: ${errorMessage(reason)}`);
                  });
                }}
                onSave={() => {
                  void mailClient.saveAttachment(message.id, attachment.id).catch((reason: unknown) => {
                    onNotice(`Could not save attachment: ${errorMessage(reason)}`);
                  });
                }}
              />
            ))}
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

/**
 * A From or To name. For other people it opens a contact card on hover or
 * focus, and clicking it makes them the subject of the context panel. The
 * user's own addresses, and readers without the panel, keep the plain
 * address-and-copy popover.
 */
function MessageAddress({ address, displayName, own }: { address: string; displayName?: string; own: Set<string> }) {
  const actions = useContext(ContactCardContext);
  const email = parseAddress(address).email.toLocaleLowerCase();
  if (!actions || !email.includes("@") || own.has(email)) return <AddressWithCopy address={address} displayName={displayName} />;
  return <AddressWithCard email={email} displayName={displayName} selected={actions.selectedEmail === email} hint="Shows this person in the context panel" onActivate={() => actions.onSelectPerson(email)} onOpenContact={actions.onOpenContact} />;
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
          {copied ? <Check size={ICON_SIZE.xs} /> : <Copy size={ICON_SIZE.xs} />}
        </button>
      </span>
    </span>
  );
}

function messageSnippet(bodyText: string, maxLength = 140): string {
  const collapsed = decodeHtmlEntities(bodyText).replace(/\s+/g, " ").trim();
  return collapsed.length > maxLength ? `${collapsed.slice(0, maxLength).trimEnd()}…` : collapsed;
}

type RecipientGroup = { label: "to" | "cc"; addresses: string[] };

/**
 * The header's "to …, cc …" groups, showing at most `limit` addresses across
 * both. Empty groups are omitted, including when no recipients are disclosed.
 */
function visibleRecipientGroups(message: Message, limit: number): { groups: RecipientGroup[]; hiddenCount: number } {
  const to = splitAddressList(message.recipients.join(", "));
  const cc = splitAddressList((message.cc ?? []).join(", "));
  const shownTo = to.slice(0, limit);
  const shownCc = cc.slice(0, Math.max(0, limit - shownTo.length));
  const groups: RecipientGroup[] = [];
  if (shownTo.length) groups.push({ label: "to", addresses: shownTo });
  if (shownCc.length) groups.push({ label: "cc", addresses: shownCc });
  return { groups, hiddenCount: to.length + cc.length - shownTo.length - shownCc.length };
}

function recipientListSeparator(index: number, recipientCount: number): string {
  if (index === 0) return "";
  if (index === recipientCount - 1) return recipientCount === 2 ? " and " : ", and ";
  return ", ";
}
