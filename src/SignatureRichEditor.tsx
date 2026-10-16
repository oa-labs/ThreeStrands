import { useEffect, useRef, type HTMLAttributes, type RefObject } from "react";
import type { Config } from "jodit/esm/config.js";
import { Jodit } from "jodit";
import "jodit/esm/plugins/justify/justify.js";
import "jodit/es2021/jodit.min.css";

type Props = HTMLAttributes<HTMLDivElement> & {
  initialHtml: string;
  editorRef: RefObject<HTMLDivElement | null>;
  instanceRef: RefObject<Jodit | null>;
  busy: boolean;
  onLink(): void;
  onImage(): void;
};

/** Jodit owns the editable DOM; React never sends its value back on keystrokes. */
export function SignatureRichEditor({ initialHtml, editorRef, instanceRef, busy, onLink, onImage, onKeyDown, onPaste, ...events }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const source = useRef<HTMLTextAreaElement>(null);
  const actions = useRef({ onLink, onImage });
  useEffect(() => { actions.current = { onLink, onImage }; });
  useEffect(() => {
    if (!source.current || !host.current) return;
    const popupRoot = host.current;
    source.current.value = initialHtml;
    // Jodit's recursive DeepPartial config exceeds TypeScript's instantiation
    // depth. Check these options against its shallow public Config instead.
    const makeEditor = Jodit.make.bind(Jodit) as (source: HTMLElement, config: Partial<Config>) => Jodit;
    const instance = makeEditor(source.current, {
      language: "en", theme: "default", autofocus: false,
      toolbarAdaptive: false, toolbarSticky: false, toolbarButtonSize: "small",
      statusbar: false, showCharsCounter: false, showWordsCounter: false,
      showXPathInStatusbar: false, placeholder: "", spellcheck: true,
      height: "auto", minHeight: 0, enter: "br",
      popupRoot,
      // ThreeStrands handles HTML insertion, links and images. Disable Jodit's
      // layout normalization and image UI so none can drop empty sender cells,
      // request remote URLs or move a dialog outside the application's modal.
      disablePlugins: ["image", "link", "inline-popup", "wrap-nodes", "dtd", "stat", "powered-by-jodit", "placeholder", "iframe"],
      cleanHTML: { ...Jodit.defaultOptions.cleanHTML, fillEmptyParagraph: false, removeEmptyElements: false, replaceNBSP: false, replaceOldTags: false },
      i18n: { en: { "Font family": "Font", "Font size": "Text Size", "Fill color or set the text color": "Text Color" } },
      buttons: ["bold", "italic", "underline", "|", "font", "fontsize", "brush", "align", "|",
        { name: "signatureLink", icon: "link", tooltip: "Insert Link", exec: () => actions.current.onLink() },
        { name: "signatureImage", icon: "image", tooltip: "Add Image…", exec: () => actions.current.onImage() },
        "|", "undo", "redo"],
    });
    instanceRef.current = instance;
    editorRef.current = instance.editor as HTMLDivElement;
    instance.editor.classList.add("signature-content");
    instance.editor.setAttribute("role", "textbox");
    instance.editor.setAttribute("aria-label", "Signature Content");
    instance.editor.setAttribute("aria-multiline", "true");
    instance.editor.setAttribute("tabindex", "0");
    const closePopup = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !host.current?.querySelector(".jodit-popup")) return;
      event.preventDefault(); event.stopPropagation();
      instance.e.fire("closeAllPopups"); instance.editor.focus();
    };
    popupRoot.addEventListener("keydown", closePopup, true);
    return () => {
      editorRef.current = null; instanceRef.current = null;
      popupRoot.removeEventListener("keydown", closePopup, true);
      instance.destruct();
    };
  }, [initialHtml, editorRef, instanceRef]);
  useEffect(() => { instanceRef.current?.setReadOnly(busy); }, [busy, instanceRef]);
  return <div {...events} className="signature-rich-editor" ref={host}
    onKeyDownCapture={(event) => { if (editorRef.current?.contains(event.target as Node)) onKeyDown?.(event); }}
    onPasteCapture={(event) => { if (editorRef.current?.contains(event.target as Node)) onPaste?.(event); }}>
    <textarea ref={source} hidden aria-hidden="true" />
  </div>;
}
