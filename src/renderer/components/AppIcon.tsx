import { appColor } from '@/renderer/lib/colors';

/** The app's real icon when macOS has one, otherwise a letter on the app's colour. */
export function AppIcon({ name, src, size = 20 }: { name: string; src?: string | null; size?: number }) {
  if (src) return <img className="fo-app-icon" src={src} alt="" width={size} height={size} draggable={false} />;
  return (
    <span className="fo-app-badge" style={{ background: appColor(name), width: size, height: size }} aria-hidden="true">
      {name.slice(0, 1)}
    </span>
  );
}
