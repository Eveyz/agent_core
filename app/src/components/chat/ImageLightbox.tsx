import { useEffect } from 'react';
import { createPortal } from 'react-dom';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';

export function ImageLightbox({
  src,
  onClose,
}: {
  src: string | null;
  onClose: () => void;
}) {
  useEffect(() => {
    if (!src) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    window.addEventListener('keydown', onKey);
    return () => {
      document.body.style.overflow = prevOverflow;
      window.removeEventListener('keydown', onKey);
    };
  }, [src, onClose]);

  if (!src) return null;

  return createPortal(
    <div className="image-lightbox-backdrop" onClick={onClose} role="presentation">
      <div
        className="image-lightbox-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Image preview"
        onClick={(e) => e.stopPropagation()}
      >
        <button type="button" className="image-lightbox-close" onClick={onClose} aria-label="Close">
          <XIcon size={16} />
        </button>
        <img src={src} alt="Attachment preview" className="image-lightbox-img" />
      </div>
    </div>,
    document.body,
  );
}
