import { mailClient } from "./data/client";
import { SIGNATURE_LIMITS } from "./emailRenderingPolicy";
import { readSignatureHtml, validateSignature } from "./signatures";

export function remoteImageSources(html: string): string[] {
  // Parse in an inert template; sender URLs never enter live app markup.
  const template = document.createElement("template"); template.innerHTML = html;
  return [...new Set(Array.from(template.content.querySelectorAll("img")).map((image) => image.getAttribute("src") ?? "").filter((src) => /^https?:\/\//i.test(src)))];
}

/**
 * Replaces each remote image source with a data URI downloaded through the
 * native proxy, which applies its URL validation, resource bounds and
 * blocked-source behavior. Requests run one at a time and CSS URLs are never
 * fetched. Images that fail keep their source, so users can retain them hosted.
 */
export async function embedRemoteImages(html: string): Promise<{ html: string; failures: unknown[] }> {
  const template = document.createElement("template"); template.innerHTML = html;
  const failures: unknown[] = [];
  for (const url of remoteImageSources(html)) {
    try {
      const data = await mailClient.fetchRemoteImage(url);
      if (dataUriBytes(data) > SIGNATURE_LIMITS.maxImageBytes) throw new Error("This image is larger than 128 KiB.");
      template.content.querySelectorAll("img").forEach((image) => { if (image.getAttribute("src") === url) image.src = data; });
    } catch (reason) { failures.push(reason); }
  }
  return { html: template.innerHTML, failures };
}

/** Decoded size of a base64 data URI's payload. */
function dataUriBytes(uri: string): number {
  const encoded = uri.slice(uri.indexOf(",") + 1);
  return Math.floor(encoded.length * 3 / 4) - (encoded.endsWith("==") ? 2 : encoded.endsWith("=") ? 1 : 0);
}

export function signatureImportContent(source: string) {
  return readSignatureHtml(source);
}

export async function prepareSignatureImport(source: string, embedRemote: boolean) {
  let html = source;
  if (embedRemote) {
    const result = await embedRemoteImages(source);
    if (result.failures.length) throw result.failures[0];
    html = result.html;
  }
  const content = signatureImportContent(html);
  validateSignature({ id: "preview", name: "Imported", ...content });
  return content;
}
