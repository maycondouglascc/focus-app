import {
  Children,
  isValidElement,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { Icon } from '@/renderer/components/Icon';

interface Opt {
  value: string;
  label: string;
  group?: string;
}

const textOf = (node: ReactNode): string =>
  Children.toArray(node)
    .map((c) => (typeof c === 'string' || typeof c === 'number' ? String(c) : ''))
    .join('');

/** Reads <option> / <optgroup> children, so call sites keep the native-select shape. */
function readOptions(children: ReactNode, group?: string, out: Opt[] = []): Opt[] {
  Children.forEach(children, (child) => {
    if (!isValidElement(child)) return;
    const el = child as ReactElement<{ value?: string | number; label?: string; children?: ReactNode }>;
    if (el.type === 'optgroup') readOptions(el.props.children, el.props.label, out);
    else if (el.type === 'option') {
      const label = textOf(el.props.children);
      out.push({ value: String(el.props.value ?? label), label, group });
    } else if (el.props.children) readOptions(el.props.children, group, out);
  });
  return out;
}

export interface LoSelectProps {
  value: string;
  onChange: (e: { target: { value: string } }) => void;
  children: ReactNode;
  id?: string;
  className?: string;
  disabled?: boolean;
  'aria-label'?: string;
  /** Shown when the value matches no option. */
  placeholder?: string;
  /** Drawn inside the trigger before the label, e.g. the chosen project's dot. */
  leading?: ReactNode;
}

/**
 * The app's one dropdown. Same look as the right-click / "…" menus (white card, soft shadow, rounded rows), with a check
 * on the current choice. Takes <option> / <optgroup> children and calls onChange like a native select, so it drops in
 * wherever a <select> was. Keyboard: Enter / Space / ↓ opens, ↑ ↓ move, Enter picks, Esc closes.
 */
export function LoSelect({
  value,
  onChange,
  children,
  id,
  className = '',
  disabled,
  placeholder,
  leading,
  ...rest
}: LoSelectProps) {
  const options = useMemo(() => readOptions(children), [children]);
  const current = options.find((o) => o.value === value);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number; minWidth: number; up: boolean } | null>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const close = useCallback((refocus = false) => {
    setOpen(false);
    if (refocus) btnRef.current?.focus();
  }, []);

  // Place under the trigger (above when there is no room), inside the window.
  useLayoutEffect(() => {
    if (!open || !btnRef.current) return;
    const r = btnRef.current.getBoundingClientRect();
    const menuH = menuRef.current?.offsetHeight ?? Math.min(options.length * 36 + 12, 320);
    const menuW = Math.max(menuRef.current?.offsetWidth ?? 0, r.width);
    const up = r.bottom + 6 + menuH > window.innerHeight - 8 && r.top - 6 - menuH > 8;
    setPos({
      left: Math.max(8, Math.min(r.left, window.innerWidth - menuW - 8)),
      top: up ? r.top - 6 - menuH : r.bottom + 6,
      minWidth: r.width,
      up,
    });
  }, [open, options.length]);

  useEffect(() => {
    if (!open) return;
    const items = () => [...(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role=option]') ?? [])];
    (items().find((b) => b.dataset.value === value) ?? items()[0])?.focus();
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!menuRef.current?.contains(t) && !btnRef.current?.contains(t)) close();
    };
    const onScroll = (e: Event) => {
      if (!menuRef.current?.contains(e.target as Node)) close();
    };
    const onBlur = () => close();
    // Esc closes the list only: stop it before a surrounding dialog's own Esc handler sees it.
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopImmediatePropagation();
      close(true);
    };
    window.addEventListener('keydown', onEsc, true);
    window.addEventListener('mousedown', onDown, true);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('blur', onBlur);
    window.addEventListener('resize', onBlur);
    return () => {
      window.removeEventListener('keydown', onEsc, true);
      window.removeEventListener('mousedown', onDown, true);
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('blur', onBlur);
      window.removeEventListener('resize', onBlur);
    };
  }, [open, value, close]);

  const pick = (v: string) => {
    close(true);
    if (v !== value) onChange({ target: { value: v } });
  };

  let lastGroup: string | undefined;
  return (
    <>
      <button
        ref={btnRef}
        id={id}
        type="button"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={rest['aria-label']}
        className={`lo-select lo-input lo-selectbtn ${className}`}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        {leading}
        <span className={current ? undefined : 'lo-selectbtn-ph'}>{current?.label ?? placeholder ?? ''}</span>
      </button>
      {open &&
        createPortal(
          <div
            ref={menuRef}
            className="lo-menu lo-selectmenu"
            role="listbox"
            style={{
              left: pos?.left ?? 0,
              top: pos?.top ?? 0,
              minWidth: pos?.minWidth,
              visibility: pos ? 'visible' : 'hidden',
              transformOrigin: pos?.up ? 'bottom left' : 'top left',
            }}
            onKeyDown={(e) => {
              const btns = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role=option]') ?? [])];
              const i = btns.indexOf(document.activeElement as HTMLButtonElement);
              if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                e.preventDefault();
                btns[(i + (e.key === 'ArrowDown' ? 1 : -1) + btns.length) % btns.length]?.focus();
              } else if (e.key === 'Home' || e.key === 'End') {
                e.preventDefault();
                btns[e.key === 'Home' ? 0 : btns.length - 1]?.focus();
              } else if (e.key === 'Tab') close();
            }}
          >
            {options.map((o) => {
              const head = o.group && o.group !== lastGroup ? <div className="lo-menu-group">{o.group}</div> : null;
              lastGroup = o.group;
              const on = o.value === value;
              return (
                <div key={o.value} role="presentation">
                  {head}
                  <button
                    type="button"
                    role="option"
                    aria-selected={on}
                    data-value={o.value}
                    className="lo-menu-item lo-menu-item--opt"
                    onClick={() => pick(o.value)}
                  >
                    <span>{o.label}</span>
                    {on && <Icon name="checkmark" size={16} aria-hidden="true" />}
                  </button>
                </div>
              );
            })}
          </div>,
          document.body,
        )}
    </>
  );
}
