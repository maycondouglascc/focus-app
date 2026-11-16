import { useId, type SVGProps } from 'react';

/**
 * SF Symbols, exported by scripts/sf-symbols.swift (`npm run icons`) as black PNGs in assets/sf.
 * Rendered as an <svg> whose fill is masked by the symbol, so it takes `currentColor` and the existing
 * `… svg { width; height; color }` rules keep sizing it.
 */
const files = import.meta.glob<string>('../assets/sf/*.png', { eager: true, query: '?url', import: 'default' });
const urls: Record<string, string> = {};
for (const [path, url] of Object.entries(files)) urls[path.slice(path.lastIndexOf('/') + 1, -4)] = url;

export type IconName =
  | 'chevron.left'
  | 'chevron.right'
  | 'plus'
  | 'stop.fill'
  | 'play.fill'
  | 'checkmark'
  | 'gearshape'
  | 'waveform.path.ecg'
  | 'pencil'
  | 'dollarsign.circle'
  | 'folder'
  | 'square.and.arrow.down'
  | 'trash'
  | 'calendar'
  | 'chart.bar'
  | 'macwindow'
  | 'globe'
  | 'terminal'
  | 'lock'
  | 'exclamationmark.triangle'
  | 'arrow.clockwise';

type Props = Omit<SVGProps<SVGSVGElement>, 'name'> & { name: IconName; size?: number };

export function Icon({ name, size = 16, ...rest }: Props) {
  const id = useId();
  return (
    <svg viewBox="0 0 20 20" width={size} height={size} aria-hidden="true" {...rest}>
      <mask id={id} style={{ maskType: 'alpha' }}>
        <image href={urls[name]} width="20" height="20" />
      </mask>
      <rect width="20" height="20" fill="currentColor" mask={`url(#${id})`} />
    </svg>
  );
}
