import { Check, ChevronDown, ChevronUp, Copy, Download, ExternalLink, Forward, Paperclip, Reply, ReplyAll } from "lucide-react";
import { memo, useCallback, useContext, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type FocusEvent, type KeyboardEvent, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { HoverTooltip } from "./AppChrome";
import { CalendarAttachmentGroup, isCalendarAttachment } from "./CalendarAttachment";
import { ContactCard, ContactCardContext, useContactLookup, type ContactCardActions } from "./ContactCard";
import type { OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account, Message } from "./domain";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import { errorMessage } from "./errors";
import { isInlineImageAttachment, normalizeContentId, referencedImageContentIds } from "./inlineAttachments";
import { decodeHtmlEntities, SafeMessage } from "./SafeMessage";
import type { ThreadTextIndex } from "./quotedHistory";
import type { FontFamily } from "./settings";
import { formatAttachmentSize, formatMailTimestamp, splitAttachmentName } from "./threadPresentation";
import { ICON_SIZE } from "./iconSizes";

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
  /** The conversation's text index; this card folds text repeated from messages before `index`. */
  threadText?: ThreadTextIndex;
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
  threadText,
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
  const priorThreadText = useMemo(() => threadText?.before(index), [threadText, index]);
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
            {downloadableAttachments.length > 0 ? <Paperclip size={ICON_SIZE.xs} aria-label="Has attachments" /> : null}
            <time>{formatMailTimestamp(message.sentAt)}</time>
            <ChevronDown size={ICON_SIZE.sm} className="message-card-chevron" />
          </button>
        </header>
        <div id={cardBodyId} hidden />
      </article>
    );
  }

  const recipients = splitAddressList(message.recipients.join(", "));
  const ownEmails = new Set(accounts.map((account) => account.email.toLocaleLowerCase()));
  const headerDetails = (
    <div className="message-header-details">
      <div className="message-sender-row">
        <strong><MessageAddress address={message.sender} displayName={senderDisplayName} own={ownEmails} /></strong>
        {queuedItem ? null : (
          <div className="message-header-actions">
            <HoverTooltip label="Reply" placement="bottom">
              <button
                type="button"
                className="btn-icon btn-icon-sm"
                aria-label="Reply"
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
                onClick={() => onRespond("forward", message.id)}
              >
                <Forward size={ICON_SIZE.sm} />
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
            <MessageAddress
              address={recipient}
              displayName={formatDisplayName(parseAddress(recipient).name)}
              own={ownEmails}
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
          priorThreadText={priorThreadText}
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
                    <Paperclip size={ICON_SIZE.sm} />
                    <span className="attachment-name">
                      <span className="attachment-name-base">{attachmentName.base}</span>
                      {attachmentName.extension ? <span className="attachment-name-ext">{attachmentName.extension}</span> : null}
                    </span>
                    <small>{formatAttachmentSize(attachment.size)}</small>
                    <ExternalLink size={ICON_SIZE.xs} />
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
                    <Download size={ICON_SIZE.sm} />
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
  return <AddressWithCard email={email} displayName={displayName} actions={actions} />;
}

/** Gap between a name and its card, and the margin kept from the window edges. */
const HOVER_CARD_GAP = 8;
const HOVER_CARD_MARGIN = 8;
/** How long a card stays open after the pointer leaves, so it can cross the gap into the card. */
const HOVER_CARD_CLOSE_DELAY_MS = 150;

/**
 * Where a hover card goes in the window: below the anchor, or above when it
 * would run off the bottom and there is more room there, and moved left so it
 * stays inside the right edge.
 */
export function placeHoverCard(
  anchor: { top: number; bottom: number; left: number },
  card: { width: number; height: number },
  viewport: { width: number; height: number },
): { top: number; left: number } {
  const below = anchor.bottom + HOVER_CARD_GAP;
  const above = anchor.top - HOVER_CARD_GAP - card.height;
  const fitsBelow = below + card.height <= viewport.height - HOVER_CARD_MARGIN;
  const top = fitsBelow || viewport.height - anchor.bottom >= anchor.top ? below : Math.max(HOVER_CARD_MARGIN, above);
  const left = Math.max(HOVER_CARD_MARGIN, Math.min(anchor.left, viewport.width - HOVER_CARD_MARGIN - card.width));
  return { top, left };
}

function AddressWithCard({ email, displayName, actions }: {
  email: string;
  displayName?: string;
  actions: ContactCardActions;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const anchorRef = useRef<HTMLSpanElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<number | null>(null);
  const hintId = useId();
  const lookup = useContactLookup(email, open);
  const name = displayName || email;
  const selected = actions.selectedEmail === email;

  const cancelClose = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };
  const show = () => { cancelClose(); setOpen(true); };
  const hideSoon = () => {
    cancelClose();
    closeTimer.current = window.setTimeout(() => { closeTimer.current = null; setOpen(false); }, HOVER_CARD_CLOSE_DELAY_MS);
  };
  const closeOnFocusOut = (event: FocusEvent) => {
    const next = event.relatedTarget as Node | null;
    if (anchorRef.current?.contains(next) || cardRef.current?.contains(next)) return;
    cancelClose();
    setOpen(false);
  };
  useEffect(() => cancelClose, []);

  // The card renders at the top of the page so the reader's scroll area can't
  // clip it and neighboring panes can't cover it; place it beside the name.
  useLayoutEffect(() => {
    if (!open) { setPosition(null); return; }
    const place = () => {
      const anchor = anchorRef.current?.getBoundingClientRect();
      const card = cardRef.current?.getBoundingClientRect();
      if (!anchor || !card) return;
      setPosition(placeHoverCard(anchor, card, { width: window.innerWidth, height: window.innerHeight }));
    };
    place();
    // A fixed card would drift from its name when the reader scrolls.
    const close = () => { cancelClose(); setOpen(false); };
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    const observer = typeof ResizeObserver === "undefined" || !cardRef.current ? null : new ResizeObserver(place);
    if (observer && cardRef.current) observer.observe(cardRef.current);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
      observer?.disconnect();
    };
  }, [open]);

  return (
    <span
      ref={anchorRef}
      className={`address address-contact${selected ? " address-selected" : ""}`}
      onClick={(event) => event.stopPropagation()}
      onMouseEnter={show}
      onMouseLeave={hideSoon}
      onFocus={show}
      onBlur={closeOnFocusOut}
    >
      <button type="button" className="address-name" aria-describedby={hintId} onClick={() => actions.onSelectPerson(email)}>{name}</button>
      <span id={hintId} hidden>Shows this person in the context panel</span>
      {open ? createPortal(
        <div
          ref={cardRef}
          className="address-card"
          role="group"
          aria-label={`Contact card for ${name}`}
          style={position ? { top: position.top, left: position.left } : { visibility: "hidden" }}
          onMouseEnter={show}
          onMouseLeave={hideSoon}
          onBlur={closeOnFocusOut}
          onClick={(event) => event.stopPropagation()}
        >
          <ContactCard
            email={email}
            fallbackName={displayName}
            profile={lookup.profile}
            loaded={lookup.loaded}
            facts={lookup.facts}
            titleAs="div"
            onOpenContact={actions.onOpenContact}
            onProfileSaved={(saved) => { lookup.setError(null); lookup.setProfile(saved); }}
            onError={lookup.setError}
          />
          {lookup.error ? <span className="contacts-error" role="alert">{lookup.error}</span> : null}
        </div>,
        document.body,
      ) : null}
    </span>
  );
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

function recipientListSeparator(index: number, recipientCount: number): string {
  if (index === 0) return "";
  if (index === recipientCount - 1) return recipientCount === 2 ? " and " : ", and ";
  return ", ";
}
