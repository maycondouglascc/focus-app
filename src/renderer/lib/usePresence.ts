import { useEffect, useState } from 'react';

/**
 * Keeps the last non-null value rendered for `ms` after it goes null, so a closing card can play its exit.
 * Returns [what to render, whether it is on its way out].
 */
export function usePresence<T>(value: T | null, ms = 140): [T | null, boolean] {
  const [kept, setKept] = useState<T | null>(value);
  useEffect(() => {
    if (value !== null) {
      setKept(value);
      return;
    }
    const reduce = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    const id = setTimeout(() => setKept(null), reduce ? 0 : ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return value !== null ? [value, false] : [kept, kept !== null];
}
