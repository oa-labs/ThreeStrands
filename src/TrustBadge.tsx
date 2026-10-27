import { useEffect, useId, useLayoutEffect, useRef, useState, type FocusEvent, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { ShieldCheck, TriangleAlert } from "lucide-react";
import { placeHoverCard } from "./AddressWithCard";
import { ICON_SIZE } from "./iconSizes";
import { SenderFeedback, TrustDetails, trustRow } from "./SenderSections";
import { trustHeadline, type TrustLink, type TrustReport } from "./senderSignals";

/** How long the details stay open after the pointer leaves, so it can cross into them. */
const TRUST_CARD_CLOSE_DELAY_MS = 150;

/**
 * The trust verdict beside the sender's name. A known sender gets a quiet
 * shield; a message that warns gets its first warning in words, unless the
 * banner above the body already says it, when the icon alone shows. Hovering
 * shows every row and the folded Details, and clicking keeps them open until
 * the user clicks again, presses Escape, or moves focus away. A warning's
 * details also offer the account-scoped trust action.
 */
export function TrustBadge({ messageId, report, onShowLink }: { messageId: string; report: TrustReport; onShowLink?(link: TrustLink): void }) {
  const [hovered, setHovered] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const anchorRef = useRef<HTMLSpanElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<number | null>(null);
  const cardId = useId();
  const open = hovered || pinned;

  const cancelClose = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };
  const show = () => { cancelClose(); setHovered(true); };
  const hideSoon = () => {
    cancelClose();
    closeTimer.current = window.setTimeout(() => { closeTimer.current = null; setHovered(false); }, TRUST_CARD_CLOSE_DELAY_MS);
  };
  const close = () => { cancelClose(); setHovered(false); setPinned(false); };
  const closeOnFocusOut = (event: FocusEvent) => {
    const next = event.relatedTarget as Node | null;
    if (anchorRef.current?.contains(next) || cardRef.current?.contains(next)) return;
    close();
  };
  const closeOnEscape = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !open) return;
    event.stopPropagation();
    close();
  };
  useEffect(() => cancelClose, []);

  // Rendered at the top of the page, like the contact card, so the reader can't clip it.
  useLayoutEffect(() => {
    if (!open) { setPosition(null); return; }
    const place = () => {
      const anchor = anchorRef.current?.getBoundingClientRect();
      const card = cardRef.current?.getBoundingClientRect();
      if (!anchor || !card) return;
      setPosition(placeHoverCard(anchor, card, { width: window.innerWidth, height: window.innerHeight }));
    };
    place();
    const dismiss = () => { cancelClose(); setHovered(false); setPinned(false); };
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    const observer = typeof ResizeObserver === "undefined" || !cardRef.current ? null : new ResizeObserver(place);
    if (observer && cardRef.current) observer.observe(cardRef.current);
    return () => {
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("resize", dismiss);
      observer?.disconnect();
    };
  }, [open]);

  const headline = trustHeadline(report);
  if (report.summary === null) return null;
  const careful = report.summary === "careful";
  const label = careful ? "Be careful" : "Known sender";
  const bannered = strongWarnings(report).length > 0;
  return (
    <span
      ref={anchorRef}
      className="message-trust"
      onClick={(event) => event.stopPropagation()}
      onMouseEnter={show}
      onMouseLeave={hideSoon}
      onBlur={closeOnFocusOut}
      onKeyDown={closeOnEscape}
    >
      <button
        type="button"
        className={`message-trust-badge ${careful ? "message-trust-careful" : "message-trust-known"}`}
        aria-label={careful ? `Be careful: ${headline}. Show sender details` : "Known sender. Show sender details"}
        aria-expanded={pinned}
        aria-controls={open ? cardId : undefined}
        onClick={() => { cancelClose(); setPinned(!pinned); }}
      >
        {careful ? <TriangleAlert size={ICON_SIZE.xs} aria-hidden="true" /> : <ShieldCheck size={ICON_SIZE.xs} aria-hidden="true" />}
        {careful && !bannered ? <span className="message-trust-headline">{headline}</span> : null}
      </button>
      {open ? createPortal(
        <div
          ref={cardRef}
          id={cardId}
          className="address-card message-trust-card"
          role="group"
          aria-label={`Sender details: ${label}`}
          style={position ? { top: position.top, left: position.left } : { visibility: "hidden" }}
          onMouseEnter={show}
          onMouseLeave={hideSoon}
          onBlur={closeOnFocusOut}
          onKeyDown={closeOnEscape}
          onClick={(event) => event.stopPropagation()}
        >
          <div className={`context-trust-summary ${careful ? "context-trust-careful" : "context-trust-known"}`}>
            {careful ? <TriangleAlert size={ICON_SIZE.xs} aria-hidden="true" /> : <ShieldCheck size={ICON_SIZE.xs} aria-hidden="true" />}
            {label}
          </div>
          {report.rows.map((finding) => trustRow(finding, onShowLink ? (link) => { close(); onShowLink(link); } : undefined))}
          <TrustDetails details={report.details} />
          {careful && report.feedback ? <SenderFeedback key={messageId} messageId={messageId} feedback={report.feedback} /> : null}
        </div>,
        document.body,
      ) : null}
    </span>
  );
}

/**
 * The strongest warnings above the message body, where they can't be missed:
 * a borrowed name, a look-alike sender or link domain, or a risky attachment.
 * Weaker warnings stay on the header badge. A borrowed name can be answered
 * here with the same account-scoped trust action as the badge details,
 * which quiets that warning for this exact delivery.
 */
export function TrustBanner({ messageId, report, onShowLink }: { messageId: string; report: TrustReport; onShowLink?(link: TrustLink): void }) {
  const strong = strongWarnings(report);
  if (strong.length === 0) return null;
  const confirmable = report.feedback && strong.some((finding) => finding.key === "impersonation");
  return (
    <div className="notice notice--warning message-trust-banner" role="note" aria-label="Be careful">
      <TriangleAlert size={ICON_SIZE.sm} aria-hidden="true" />
      <div className="message-trust-banner-body">
        <ul className="message-trust-banner-list">
          {strong.map((finding) => {
            const link = finding.link;
            return (
              <li key={finding.key}>
                <span className="message-trust-banner-title">{finding.title}</span>
                {finding.addressComparison ? <div className="message-trust-address-comparison">
                  <span className="message-trust-sender-name">{finding.addressComparison.name}</span>
                  <dl>
                    <div><dt>Sending address</dt><dd>{finding.addressComparison.sending}</dd></div>
                    <div><dt>Previously seen</dt><dd>{finding.addressComparison.previous.join(", ")}</dd></div>
                  </dl>
                </div> : finding.detail ? <span className="message-trust-banner-detail">{finding.detail}</span> : null}
                {link && onShowLink ? <button type="button" className="btn-link" onClick={() => onShowLink(link)}>Show link</button> : null}
              </li>
            );
          })}
        </ul>
        {confirmable && report.feedback ? <SenderFeedback key={messageId} messageId={messageId} feedback={report.feedback} compact /> : null}
      </div>
    </div>
  );
}

function strongWarnings(report: TrustReport) {
  return report.rows.filter((finding) => finding.tone === "warning" && finding.strong);
}
