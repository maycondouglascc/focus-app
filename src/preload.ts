import { contextBridge, ipcRenderer } from 'electron';
import { Channels, type ChangeScope, type FocusApi } from './shared/ipc';

const call =
  <A extends unknown[], R>(ch: string) =>
  (...a: A) =>
    ipcRenderer.invoke(ch, ...a) as Promise<R>;

const api: FocusApi = {
  projects: {
    list: call(Channels.projectList),
    create: call(Channels.projectCreate),
    update: call(Channels.projectUpdate),
    remove: call(Channels.projectDelete),
  },
  entries: {
    list: call(Channels.entryList),
    create: call(Channels.entryCreate),
    update: call(Channels.entryUpdate),
    remove: call(Channels.entryDelete),
    running: call(Channels.entryRunning),
  },
  timer: {
    start: call(Channels.timerStart),
    stop: call(Channels.timerStop),
  },
  activity: {
    list: call(Channels.activityList),
    remove: call(Channels.activityDelete),
    clear: call(Channels.activityClear),
  },
  apps: {
    icons: call(Channels.appIcons),
  },
  monitor: {
    status: call(Channels.monitorStatus),
    requestAccess: call(Channels.monitorRequestAccess),
  },
  settings: {
    get: call(Channels.settingsGet),
    save: call(Channels.settingsSave),
  },
  onChanged: (cb) => {
    const l = (_e: unknown, scope: ChangeScope) => cb(scope);
    ipcRenderer.on(Channels.changed, l);
    return () => ipcRenderer.removeListener(Channels.changed, l);
  },
};

contextBridge.exposeInMainWorld('focusApp', api);
// Demo mode (`npm run demo`): the renderer swaps in sample data and a fixed clock.
contextBridge.exposeInMainWorld('focusDemo', process.argv.includes('--focus-demo'));
contextBridge.exposeInMainWorld('focusPlatform', process.platform);
