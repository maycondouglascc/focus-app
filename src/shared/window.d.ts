import type { FocusApi } from './ipc';

declare global {
  interface Window {
    /** Present inside the Electron app (preload). Absent in a plain browser, where a demo store stands in. */
    focusApp?: FocusApi;
    /** True when the app was started with `npm run demo`. */
    focusDemo?: boolean;
    /** Host OS platform (`'darwin'`, `'linux'`, ...), exposed by preload. */
    focusPlatform?: NodeJS.Platform;
  }
}

export {};
