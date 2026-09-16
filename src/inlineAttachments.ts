import type { MessageAttachment } from "./domain";

export function normalizeContentId(value: string): string {
  let decoded = value;
  try {
    decoded = decodeURIComponent(value);
  } catch {
    // Preserve malformed percent escapes so a literal Content-ID can still match.
  }
  return decoded.trim().replace(/^<|>$/g, "").toLocaleLowerCase();
}

/**
 * Finds sender-authored inline-image references without treating arbitrary text
 * containing `cid:` as evidence that a downloadable attachment belongs in the
 * body. A template's document fragment is inert, so parsing cannot fetch the
 * referenced resource or execute sender content.
 */
export function referencedImageContentIds(html: string): ReadonlySet<string> {
  const template = document.createElement("template");
  template.innerHTML = html;
  const ids = new Set<string>();
  template.content.querySelectorAll<HTMLImageElement>("img[src]").forEach((image) => {
    const source = image.getAttribute("src") ?? "";
    if (!/^cid:/i.test(source)) return;
    const contentId = normalizeContentId(source.slice(4));
    if (contentId) ids.add(contentId);
  });
  return ids;
}

export function isInlineImageAttachment(
  attachment: MessageAttachment,
  referencedContentIds: ReadonlySet<string>,
): boolean {
  if (attachment.inline) return true;
  return Boolean(
    attachment.contentId
    && attachment.mimeType.split(";", 1)[0]?.trim().toLocaleLowerCase().startsWith("image/")
    && referencedContentIds.has(normalizeContentId(attachment.contentId)),
  );
}
