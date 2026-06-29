import { useCallback, useRef, useEffect } from 'react';

export function useResizableSidebar(
  initialWidth: number,
  minWidth: number,
  maxWidth: number,
  direction: 'left' | 'right'
) {
  const sidebarRef = useRef<HTMLDivElement>(null);
  const isDragging = useRef(false);
  const startX = useRef(0);
  const startWidth = useRef(initialWidth);
  const overlayRef = useRef<HTMLDivElement | null>(null);

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault(); // Block native dragging and selection on mousedown
    
    isDragging.current = true;
    startX.current = e.clientX;
    
    if (sidebarRef.current) {
      startWidth.current = sidebarRef.current.getBoundingClientRect().width;
    } else {
      startWidth.current = initialWidth;
    }

    // Robustly prevent text selection during drag
    document.body.classList.add('is-resizing');
    window.getSelection()?.removeAllRanges();

    // Create an invisible overlay to intercept all pointer events
    // This fully prevents text selection in contenteditables, textareas, or iframes
    const overlay = document.createElement('div');
    overlay.style.position = 'fixed';
    overlay.style.top = '0';
    overlay.style.left = '0';
    overlay.style.right = '0';
    overlay.style.bottom = '0';
    overlay.style.zIndex = '99999';
    overlay.style.cursor = 'col-resize';
    document.body.appendChild(overlay);
    overlayRef.current = overlay;
  }, [initialWidth]);

  const onMouseMove = useCallback((e: MouseEvent) => {
    if (!isDragging.current || !sidebarRef.current) return;
    
    e.preventDefault(); // Helps prevent text selection and default drag behaviors

    const delta = e.clientX - startX.current;
    let newWidth = startWidth.current;

    if (direction === 'left') {
      newWidth += delta;
    } else {
      newWidth -= delta;
    }

    newWidth = Math.max(minWidth, Math.min(maxWidth, newWidth));
    
    // Direct DOM manipulation for butter-smooth resizing (avoids full React tree re-render)
    sidebarRef.current.style.width = `${newWidth}px`;
  }, [direction, minWidth, maxWidth]);

  const onMouseUp = useCallback(() => {
    if (isDragging.current) {
      isDragging.current = false;
      document.body.classList.remove('is-resizing');
      
      if (overlayRef.current) {
        overlayRef.current.remove();
        overlayRef.current = null;
      }
    }
  }, []);

  useEffect(() => {
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);

    return () => {
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.classList.remove('is-resizing');
      
      if (overlayRef.current) {
        overlayRef.current.remove();
        overlayRef.current = null;
      }
    };
  }, [onMouseMove, onMouseUp]);

  return { sidebarRef, onMouseDown };
}

