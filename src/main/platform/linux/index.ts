import { gnomeWaylandMonitor } from './gnome';
import { hyprlandMonitor } from './hyprland';
import { getLinuxIdleSeconds, startLinuxLockWatcher } from './idle';
import { kwinMonitor } from './kwin';
import { swayMonitor } from './sway';
import { x11Monitor } from './x11';
import type { PlatformMonitor } from '../types';

function createUnavailableMonitor(sessionType: 'wayland' | 'x11' | 'unknown'): PlatformMonitor {
  const desktop = process.env.XDG_CURRENT_DESKTOP || 'this desktop environment';
  return {
    backend: 'none',
    sessionType,
    capabilities: {
      windowTitles: false,
      browserTabs: false,
      idleDetection: true,
      mediaActivity: false,
    },
    sample: () => Promise.resolve(null),
    getIdleSeconds: () => getLinuxIdleSeconds(false),
    getPermissionState: () =>
      Promise.resolve({
        state: 'unavailable',
        hint: `Automatic window tracking is not supported on ${desktop} (${sessionType}) yet. Manual timer and Claude Code sessions still work.`,
      }),
    requestAccess: () => Promise.resolve(false),
    startLockWatcher: startLinuxLockWatcher,
  };
}

export function createLinuxMonitor(): PlatformMonitor {
  if (process.env.SWAYSOCK) return swayMonitor;
  if (process.env.HYPRLAND_INSTANCE_SIGNATURE) return hyprlandMonitor;

  const sessionTypeEnv = (process.env.XDG_SESSION_TYPE || '').toLowerCase();
  const isWayland = sessionTypeEnv === 'wayland' || !!process.env.WAYLAND_DISPLAY;
  const desktop = `${process.env.XDG_CURRENT_DESKTOP || ''}:${process.env.DESKTOP_SESSION || ''}`.toLowerCase();

  if (isWayland) {
    if (desktop.includes('gnome') || desktop.includes('ubuntu')) return gnomeWaylandMonitor;
    if (desktop.includes('kde') || desktop.includes('plasma')) return kwinMonitor;
    return createUnavailableMonitor('wayland');
  }

  // Never fall back to X11 inside a Wayland session; XWayland would miss native Wayland windows.
  if (sessionTypeEnv === 'x11' || (!!process.env.DISPLAY && !process.env.WAYLAND_DISPLAY)) {
    return x11Monitor;
  }

  return createUnavailableMonitor('unknown');
}
