/**
 * Whether `target` is an Element. Duck-typed rather than `instanceof Element`
 * because targets inside the message iframe belong to that frame's realm, not
 * the parent's.
 */
export function isElement(target: EventTarget | null | undefined): target is Element {
  return target != null
    && "nodeType" in target && target.nodeType === Node.ELEMENT_NODE
    && "closest" in target && typeof target.closest === "function";
}

/** The closest ancestor of `target` (inclusive) matching `selector`, if any. */
export function closestFrom<E extends Element = Element>(target: EventTarget | null | undefined, selector: string): E | null {
  return isElement(target) ? target.closest<E>(selector) : null;
}
