import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

/**
 * A card that floats beside an anchor rectangle (right of it when there is room, else left), clamped inside the
 * window. Without an anchor it centres. Esc and a click outside close it.
 */
export function Popover({
  anchor,
  onClose,
  width = 340,
  children,
  label,
  closing = false,
}: {
  anchor: DOMRect | null;
  onClose: () => void;
  width?: number;
  children: ReactNode;
  label: string;
  /** Playing its exit: no longer interactive. */
  closing?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  // Once it has been shown in place, a move to another anchor glides instead of jumping. Set a frame after the first
  // placement, so the first one (from off-screen) never animates.
  const [placed, setPlaced] = useState(false);
  useEffect(() => {
    if (!pos || placed) return;
    const id = requestAnimationFrame(() => setPlaced(true));
    return () => cancelAnimationFrame(id);
  }, [pos, placed]);

  const place = useCallback(() => {
    const h = ref.current?.offsetHeight ?? 300;
    const W = window.innerWidth;
    const H = window.innerHeight;
    if (!anchor) return setPos({ left: (W - width) / 2, top: Math.max(24, (H - h) / 2) });
    const gap = 10;
    let left = anchor.right + gap;
    if (left + width > W - 12) left = anchor.left - gap - width;
    if (left < 12) left = Math.min(W - width - 12, Math.max(12, anchor.left + 24));
    const top = Math.max(12, Math.min(anchor.top - 8, H - h - 12));
    setPos({ left, top });
  }, [anchor, width]);
  useLayoutEffect(place, [place]);
  // Content that grows (an app unfolding) keeps the card inside the window.
  useEffect(() => {
    if (!ref.current) return;
    const ro = new ResizeObserver(() => place());
    ro.observe(ref.current);
    return () => ro.disconnect();
  }, [place]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (closing) return;
      if (e.key === 'Escape' && !e.defaultPrevented) {
        e.preventDefault();
        onClose();
      }
    };
    const onDown = (e: MouseEvent) => {
      if (closing) return;
      const t = e.target as HTMLElement;
      if (ref.current?.contains(t) || t.closest('.lo-menu')) return;
      // A press on the thing that opened it is left to that thing, so a second click toggles it shut.
      if (anchor && e.clientX >= anchor.left && e.clientX <= anchor.right && e.clientY >= anchor.top && e.clientY <= anchor.bottom) return;
      onClose();
    };
    window.addEventListener('keydown', onKey);
    // Deferred so the click that opened the popover does not close it.
    const id = setTimeout(() => window.addEventListener('mousedown', onDown), 0);
    return () => {
      clearTimeout(id);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onDown);
    };
  }, [onClose, anchor, closing]);

  return createPortal(
    <div
      ref={ref}
      role="dialog"
      aria-label={label}
      className="fo-pop"
      data-closing={closing}
      data-placed={placed}
      style={{
        width,
        left: pos?.left ?? -9999,
        top: pos?.top ?? 0,
        visibility: pos ? 'visible' : 'hidden',
        transformOrigin: anchor && pos ? (pos.left > anchor.left ? 'left top' : 'right top') : 'center',
      }}
    >
      {children}
    </div>,
    document.body,
  );
}
