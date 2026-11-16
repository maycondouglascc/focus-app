import path from 'node:path';
import { app, BrowserWindow, Menu, nativeImage, Tray } from 'electron';
import { entries, projects } from './db';
import { startTimer, stopTimer } from './ipc';

/** Menu-bar timer: shows the running entry's elapsed time; start/stop without opening the window. */

let tray: Tray | null = null;
let ticker: NodeJS.Timeout | undefined;

const fmt = (ms: number) => {
  const m = Math.floor(ms / 60_000);
  return `${Math.floor(m / 60)}:${String(m % 60).padStart(2, '0')}`;
};

export function refreshTray() {
  if (!tray) return;
  const running = entries.running();
  const project = running?.projectId ? projects.list().find((p) => p.id === running.projectId) : undefined;
  tray.setTitle(running ? ` ${fmt(Date.now() - running.start)}` : '', { fontType: 'monospacedDigit' });
  tray.setToolTip(running ? `${running.description || 'No description'}${project ? ` · ${project.name}` : ''}` : 'Focus');
  const recent = projects.list().filter((p) => !p.archived).slice(0, 8);
  tray.setContextMenu(
    Menu.buildFromTemplate([
      running
        ? { label: `Stop  ${running.description || project?.name || 'timer'}`, click: () => (stopTimer(), refreshTray()) }
        : { label: 'Start timer', click: () => (startTimer({ projectId: null, description: '', billable: false }), refreshTray()) },
      ...(running
        ? []
        : recent.length
          ? [
              {
                label: 'Start on project',
                submenu: recent.map((p) => ({
                  label: p.name,
                  click: () => (startTimer({ projectId: p.id, description: '', billable: p.billableByDefault }), refreshTray()),
                })),
              },
            ]
          : []),
      { type: 'separator' },
      {
        label: 'Open Focus',
        click: () => {
          const w = BrowserWindow.getAllWindows()[0];
          if (w) {
            w.show();
            w.focus();
          } else app.emit('activate');
        },
      },
      { label: 'Quit', role: 'quit' },
    ]),
  );
}

export function createTray() {
  // Template image: macOS tints it for light and dark menu bars. Ships beside app.asar (forge extraResource).
  const dir = app.isPackaged ? path.join(process.resourcesPath, 'assets') : path.join(app.getAppPath(), 'assets');
  const icon = nativeImage.createFromPath(path.join(dir, 'trayTemplate.png'));
  icon.setTemplateImage(true);
  tray = new Tray(icon);
  refreshTray();
  ticker = setInterval(refreshTray, 20_000);
}

export function destroyTray() {
  if (ticker) clearInterval(ticker);
  tray?.destroy();
  tray = null;
}
