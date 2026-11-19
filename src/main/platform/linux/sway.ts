import { resolveDesktopApp } from './desktop';
import { execText } from './exec';
import { getLinuxIdleSeconds, isLinuxFocusedMediaActive, startLinuxLockWatcher } from './idle';
import type { PlatformMonitor, RawWindowSample } from '../types';

interface SwayNode {
  focused?: boolean;
  name?: string;
  app_id?: string | null;
  pid?: number;
  window_properties?: { class?: string; instance?: string; title?: string };
  nodes?: SwayNode[];
  floating_nodes?: SwayNode[];
}

function findFocused(node: SwayNode): SwayNode | null {
  if (node.focused) return node;
  for (const child of node.nodes ?? []) {
    const hit = findFocused(child);
    if (hit) return hit;
  }
  for (const child of node.floating_nodes ?? []) {
    const hit = findFocused(child);
    if (hit) return hit;
  }
  return null;
}

export const swayMonitor: PlatformMonitor = {
  backend: 'sway',
  sessionType: 'wayland',
  capabilities: {
    windowTitles: true,
    browserTabs: false,
    idleDetection: true,
    mediaActivity: true,
  },

  async sample(): Promise<RawWindowSample | null> {
    const out = await execText('swaymsg', ['-t', 'get_tree'], 2000);
    if (!out) return null;
    let tree: SwayNode;
    try {
      tree = JSON.parse(out) as SwayNode;
    } catch {
      return null;
    }
    const focused = findFocused(tree);
    if (!focused) return null;

    const appId = focused.app_id || '';
    const wmClass = focused.window_properties?.class || focused.window_properties?.instance || '';
    const title = focused.name || focused.window_properties?.title || '';
    const rawId = appId || wmClass;
    if (!rawId && !title) return null;

    const info = resolveDesktopApp(appId, wmClass, rawId);
    return {
      app: info.name,
      bundleId: rawId || info.desktopId,
      title,
      url: '',
      tab: '',
      pid: focused.pid,
      titlesAllowed: true,
    };
  },

  getIdleSeconds() {
    return getLinuxIdleSeconds(false);
  },

  isFocusedMediaActive(sample) {
    return isLinuxFocusedMediaActive(sample, false);
  },

  async getPermissionState() {
    const ok = await execText('swaymsg', ['-t', 'get_version'], 1500);
    return ok ? { state: 'granted' } : { state: 'unavailable', hint: 'Could not reach swaymsg IPC socket.' };
  },

  async requestAccess() {
    const ok = await execText('swaymsg', ['-t', 'get_version'], 1500);
    return !!ok;
  },

  startLockWatcher: startLinuxLockWatcher,
};
