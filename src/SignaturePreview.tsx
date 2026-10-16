import { memo, useMemo } from "react";
import { SafeMessage } from "./SafeMessage";
import { mailClient } from "./data/client";
import { embeddedRasterImageSrc, hostedSignatureImageSrc } from "./emailRenderingPolicy";
import { sanitizeSignatureHtml } from "./signatures";

/** Hosted images stay blocked until Load Images; the iframe receives only proxy data. */
export const SignaturePreview = memo(function SignaturePreview({ html, text }: { html: string; text: string }) {
  const preview = useMemo(() => {
    const template = document.createElement("template"); template.innerHTML = sanitizeSignatureHtml(html);
    const namespace = crypto.randomUUID();
    const embedded = new Map<string, string>();
    template.content.querySelectorAll("img").forEach((image, index) => {
      const source = image.getAttribute("src") ?? "";
      if (!embeddedRasterImageSrc.test(source)) return;
      // Local CIDs use the reader's automatic embedded-image path, while HTTPS
      // sources remain behind its existing blocked-source and native proxy flow.
      const cid = `cid:signature-preview-${namespace}-${index}`;
      embedded.set(cid, source); image.setAttribute("src", cid);
    });
    return { html: template.innerHTML, embedded, namespace };
  }, [html]);
  async function resolveImage(source: string): Promise<string> {
    const embedded = preview.embedded.get(source);
    if (embedded) return embedded;
    if (hostedSignatureImageSrc(source)) return mailClient.fetchRemoteImage(source);
    throw new Error("Unsupported signature image source.");
  }
  return <div className="signature-preview-paper"><SafeMessage theme="light" html={preview.html} text={text} loadImages={false} imageCacheKey={preview.namespace} resolveImage={resolveImage} foldQuotes={false} /></div>;
});
