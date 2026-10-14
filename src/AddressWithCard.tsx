import { useEffect, useId, useLayoutEffect, useRef, useState, type FocusEvent } from "react";
import { createPortal } from "react-dom";
import { ContactCard, useContactLookup } from "./ContactCard";

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

/**
 * A person's name that opens their contact card on hover or focus. Clicking
 * the name runs `onActivate`, which `hint` describes to screen readers.
 */
export function AddressWithCard({ email, displayName, selected = false, hint, onActivate, onOpenContact }: {
  email: string;
  displayName?: string;
  selected?: boolean;
  hint: string;
  onActivate(): void;
  onOpenContact(id: string): void;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const anchorRef = useRef<HTMLSpanElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<number | null>(null);
  const hintId = useId();
  const lookup = useContactLookup(email, open);
  const name = displayName || email;

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

  // The card renders at the top of the page so a scrolling pane can't
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
    // A fixed card would drift from its name when its pane scrolls.
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
      <button type="button" className="address-name" aria-describedby={hintId} onClick={onActivate}>{name}</button>
      <span id={hintId} hidden>{hint}</span>
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
            onOpenContact={onOpenContact}
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
