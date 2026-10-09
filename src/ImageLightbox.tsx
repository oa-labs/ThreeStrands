import { X } from "lucide-react";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { ICON_SIZE } from "./iconSizes";

export function ImageLightbox({ src, onClose }: { src: string; onClose(): void }) {
  useEscapeDismiss(onClose);
  return (
    <div className="modal-backdrop lightbox-backdrop" role="presentation" onMouseDown={onClose}>
      <button type="button" className="lightbox-close" aria-label="Close" onClick={onClose}>
        <X size={ICON_SIZE.lg} />
      </button>
      <img src={src} alt="" className="lightbox-image" onMouseDown={(event) => event.stopPropagation()} />
    </div>
  );
}
