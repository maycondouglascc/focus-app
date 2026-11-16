import type { FocusApi } from './ipc';

declare global {
  interface Window {
    /** Present inside the Electron app (preload). Absent in a plain browser, where a demo store stands in. */
    focusApp?: FocusApi;
    /** True when the app was started with `npm run demo`. */
    focusDemo?: boolean;
  }
}

export {};
