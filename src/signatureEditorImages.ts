import { mailClient } from "./data/client";
import { EMAIL_IMAGE_LIMITS, embeddedRasterImageSrc, hostedSignatureImageSrc } from "./emailRenderingPolicy";
import { fitsMessageImageBudget, resolveSharedImage } from "./SafeMessage";
import { errorMessage } from "./errors";

/** Explicitly load blocked editor images through the reader's bounded proxy queue.
 * Only validated raster data reaches live markup. The original HTTPS source
 * stays in data-compose-source, so saving never embeds these preview copies.
 */
export async function loadSignatureEditorImages(editor: HTMLElement): Promise<string[]> {
  const images = Array.from(editor.querySelectorAll<HTMLImageElement>("img[data-compose-source]"));
  const sources = [...new Set(images.map((image) => image.dataset.composeSource ?? "").filter(hostedSignatureImageSrc))];
  // Existing embedded/uploaded images also occupy the shared display budget.
  const localData = new Set(Array.from(editor.querySelectorAll<HTMLImageElement>("img:not([data-compose-source])"))
    .map((image) => image.getAttribute("src") ?? "").filter((data) => embeddedRasterImageSrc.test(data)));
  const accepted = new Map<string, string>();
  for (const image of images) {
    const data = image.getAttribute("src") ?? "";
    if (embeddedRasterImageSrc.test(data)) accepted.set(image.dataset.composeSource!, data);
  }
  let bytes = [...localData, ...accepted.values()].reduce((total, data) => total + data.length, 0);
  const errors: string[] = [];
  for (const [index, source] of sources.entries()) {
    if (!editor.isConnected) break;
    if (index + localData.size >= EMAIL_IMAGE_LIMITS.maxImagesPerMessage) {
      errors.push("This signature has too many images to preview."); break;
    }
    try {
      const data = accepted.get(source) ?? await resolveSharedImage(source, source, (url) => mailClient.fetchRemoteImage(url));
      if (!editor.isConnected) break;
      if (!embeddedRasterImageSrc.test(data)) throw new Error("The image source did not return a supported image.");
      if (!accepted.has(source)) {
        if (!fitsMessageImageBudget(bytes, data.length)) throw new Error("The signature images are too large to preview together.");
        bytes += data.length; accepted.set(source, data);
      }
      for (const image of images) {
        // A pending request must not overwrite a removed or replaced image.
        if (editor.contains(image) && image.dataset.composeSource === source) image.setAttribute("src", data);
      }
    } catch (reason) { errors.push(errorMessage(reason)); }
  }
  return errors;
}
