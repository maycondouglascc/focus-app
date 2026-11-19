import type { MonitorBackendId, MonitorStatus } from '@/shared/types';

export const currentPlatform: 'darwin' | 'linux' | 'win32' =
  window.focusPlatform === 'darwin' || window.focusPlatform === 'linux' || window.focusPlatform === 'win32'
    ? window.focusPlatform
    : typeof navigator !== 'undefined' && navigator.userAgent.includes('Macintosh')
      ? 'darwin'
      : typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')
        ? 'win32'
        : 'linux';

export const isMac = currentPlatform === 'darwin';
export const isLinux = currentPlatform === 'linux';
export const isWin = currentPlatform === 'win32';
/** Linux and Windows use the native window title bar, so custom drag strips are omitted. */
export const useNativeTitleBar = !!window.focusApp && !isMac;

export function backendLabel(backend?: MonitorBackendId): string {
  switch (backend) {
    case 'gnome-wayland':
      return 'GNOME Wayland';
    case 'sway':
      return 'Sway';
    case 'hyprland':
      return 'Hyprland';
    case 'kwin':
      return 'KDE Plasma';
    case 'x11':
      return 'X11';
    case 'macos-jxa':
      return 'macOS';
    case 'win32':
      return 'Windows';
    default:
      return 'this session';
  }
}

export function browserTabsHint(platform: 'darwin' | 'linux' | 'win32' | undefined, supported: boolean): string {
  if (!supported) {
    return 'Tab URL tracking is not available on this platform yet. Focus still captures browser window titles.';
  }
  if ((platform ?? currentPlatform) === 'win32') {
    return "Reads the active tab's address in Chrome, Edge, Brave, Firefox, Arc and other browsers via Windows UI Automation.";
  }
  return "Reads the active tab's title and address in Arc, Chrome, Brave, Edge and Safari. macOS asks once per browser.";
}

export function mediaActivityHint(supported: boolean): string {
  if (supported) {
    return 'Keep counting when the focused app is playing audio or video, even without keyboard or mouse input.';
  }
  return 'Passive media detection is not available on this platform yet.';
}

export interface AccessSummary {
  allowed: boolean;
  title: string;
  hint: string;
  actionLabel: string | null;
}

export function describeAccessState(status: MonitorStatus | undefined, asked: boolean): AccessSummary {
  if (status?.accessibility) {
    return {
      allowed: true,
      title: 'Window titles are allowed',
      hint:
        (status.platform === 'linux' || status.platform === 'win32') && status.backend
          ? `Active via ${backendLabel(status.backend)}. Focus sees files, documents and window names.`
          : 'Focus can see which file, page or document was in front.',
      actionLabel: null,
    };
  }

  if (status?.platform === 'win32' || (!status && isWin)) {
    return {
      allowed: true,
      title: 'Window titles are allowed',
      hint: 'Active via Windows. Focus sees files, documents and window names without extra permissions.',
      actionLabel: null,
    };
  }

  if (status?.platform === 'linux' || (!status && isLinux)) {
    const state = status?.permissionState ?? 'needs-setup';
    if (state === 'needs-relogin') {
      return {
        allowed: false,
        title: 'Log out and back in to activate',
        hint: status?.setupHint || 'Extension installed. Log out and back in once so GNOME Shell loads it.',
        actionLabel: 'Check again',
      };
    }
    if (state === 'unavailable') {
      return {
        allowed: false,
        title: 'Window tracking unavailable',
        hint: status?.setupHint || 'Automatic window tracking is not supported in this desktop session.',
        actionLabel: null,
      };
    }
    const isGnome = status?.backend === 'gnome-wayland';
    return {
      allowed: false,
      title: isGnome ? 'Enable GNOME extension' : 'Setup required for window titles',
      hint: status?.setupHint || 'Enable window tracking so Focus can read active window titles.',
      actionLabel: isGnome ? (asked ? 'Check again' : 'Enable extension') : 'Check again',
    };
  }

  return {
    allowed: false,
    title: 'Allow window titles',
    hint: asked
      ? 'Turn on Focus in System Settings → Privacy & Security → Accessibility. This updates by itself.'
      : 'macOS asks for Accessibility access. Without it Focus only knows the app name, not the file or page.',
    actionLabel: asked ? 'Ask again' : 'Allow access',
  };
}
