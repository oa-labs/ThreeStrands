import { useMemo } from "react";
import { SafeMessage } from "./SafeMessage";
import { embeddedRasterImageSrc } from "./emailRenderingPolicy";
import { sanitizeSignatureHtml } from "./signatures";

async function resolveEmbeddedImage(source: string): Promise<string> {
  if (!embeddedRasterImageSrc.test(source)) throw new Error("Only embedded raster images can be previewed.");
  return source;
}

/**
 * Signature HTML contains local raster images after import. Sanitize before
 * enabling image display, and resolve only those data URIs locally. Remote
 * URLs, CSS URLs and active image formats never gain a loading path here;
 * downloads stay in the explicit import action and its native proxy.
 * SafeMessage retains its sandbox, CSP and shared image budgets.
 */
export function SignaturePreview({ html, text }: { html: string; text: string }) {
  const sanitized = useMemo(() => sanitizeSignatureHtml(html), [html]);
  return <SafeMessage html={sanitized} text={text} loadImages resolveImage={resolveEmbeddedImage} foldQuotes={false} />;
}
