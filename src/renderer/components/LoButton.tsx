import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { Icon, type IconName } from '@/renderer/components/Icon';

type Props = Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children'> & {
  variant?: 'secondary' | 'primary' | 'ghost' | 'danger';
  icon?: IconName;
  /** Small (28px) for in-row actions. */
  small?: boolean;
  /** Icon-only buttons must pass `aria-label`. */
  children?: ReactNode;
  keys?: string[];
};

/** The Merged design's 36px button (see styles/controls.css). */
export function LoButton({ variant = 'secondary', icon, small, keys, children, className, ...rest }: Props) {
  const cls = [
    'lo-btn',
    variant === 'primary' && 'lo-btn--primary',
    variant === 'ghost' && 'lo-btn--ghost',
    variant === 'danger' && 'lo-btn--danger',
    !children && !keys && 'lo-btn--icon',
    small && 'lo-btn--sm',
    className,
  ]
    .filter(Boolean)
    .join(' ');
  return (
    <button type="button" className={cls} {...rest}>
      {icon && <Icon name={icon} />}
      {children}
      {keys && (
        <span className="lo-keys" aria-hidden="true">
          {keys.map((k) => (
            <kbd key={k}>{k}</kbd>
          ))}
        </span>
      )}
    </button>
  );
}
