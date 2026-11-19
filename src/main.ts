import { app, BrowserWindow, nativeTheme, shell } from 'electron';
import path from 'node:path';
import { initDb, settings } from './main/db';
import { broadcast, registerHandlers } from './main/ipc';
import { startMonitor, stopMonitor } from './main/monitor';
import { startClaudeWatcher, stopClaudeWatcher } from './main/claudeSessions';
import { createTray, destroyTray, hasTray, refreshTray } from './main/tray';
import { handleSquirrelEvent } from './main/platform/win32/squirrel';

const squirrelExiting = handleSquirrelEvent();
if (squirrelExiting) {
  app.quit();
}

/** `npm run demo`: sample data in the renderer and a fixed 1440×1100 window, for screenshots. The real database is untouched. */
const DEMO = process.env.FOCUS_DEMO === '1';

const assetsDir = () => (app.isPackaged ? path.join(process.resourcesPath, 'assets') : path.join(app.getAppPath(), 'assets'));

let isQuitting = false;

const createWindow = () => {
  // Light-only interface, so the window material must be light too.
  nativeTheme.themeSource = 'light';
  const isMac = process.platform === 'darwin';
  const isWin = process.platform === 'win32';
  const win = new BrowserWindow({
    show: false,
    width: 1440,
    height: DEMO ? 1100 : 920,
    ...(DEMO ? { enableLargerThanScreen: true, center: true } : {}),
    minWidth: 1040,
    minHeight: 680,
    ...(isMac
      ? { titleBarStyle: 'hiddenInset' as const, vibrancy: 'sidebar' as const, backgroundColor: '#00000000' }
      : { autoHideMenuBar: true, backgroundColor: '#ffffff', icon: path.join(assetsDir(), isWin ? 'icon.ico' : 'icon.png') }),
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      additionalArguments: DEMO ? ['--focus-demo'] : [],
    },
  });
  win.once('ready-to-show', () => win.show());
  setTimeout(() => !win.isDestroyed() && !win.isVisible() && win.show(), 5000);

  win.on('close', (event) => {
    if ((process.platform === 'linux' || process.platform === 'win32') && !isQuitting && hasTray()) {
      event.preventDefault();
      win.hide();
    }
  });

  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:\/\//i.test(url)) void shell.openExternal(url);
    return { action: 'deny' };
  });
  win.webContents.on('will-navigate', (event, url) => {
    const appUrl = win.webContents.getURL();
    const same = MAIN_WINDOW_VITE_DEV_SERVER_URL ? url.startsWith(MAIN_WINDOW_VITE_DEV_SERVER_URL) : url.split('#')[0] === appUrl.split('#')[0];
    if (!same) event.preventDefault();
  });

  if (MAIN_WINDOW_VITE_DEV_SERVER_URL) void win.loadURL(MAIN_WINDOW_VITE_DEV_SERVER_URL);
  else void win.loadFile(path.join(__dirname, `../renderer/${MAIN_WINDOW_VITE_NAME}/index.html`));
};

// Never touch the Keychain: Chromium otherwise creates "Focus Safe Storage" there, and every new build (a new
// ad-hoc signature) makes macOS ask for the user's password to read it. Focus stores nothing secret.
app.commandLine.appendSwitch('use-mock-keychain');

if (process.platform === 'win32') {
  app.setAppUserModelId('com.focus.app');
}

if (process.platform === 'linux') {
  if (!process.env.ELECTRON_OZONE_PLATFORM_HINT && !app.commandLine.hasSwitch('ozone-platform-hint') && !app.commandLine.hasSwitch('ozone-platform')) {
    app.commandLine.appendSwitch('ozone-platform-hint', 'auto');
  }
  if (!app.commandLine.hasSwitch('password-store')) {
    app.commandLine.appendSwitch('password-store', 'basic');
  }
  app.setDesktopName('focus.desktop');
}

const gotSingleInstanceLock = !squirrelExiting && app.requestSingleInstanceLock();
if (squirrelExiting || !gotSingleInstanceLock) {
  app.quit();
} else {
  app.on('second-instance', () => {
    const win = BrowserWindow.getAllWindows()[0];
    if (win) {
      if (win.isMinimized()) win.restore();
      if (!win.isVisible()) win.show();
      win.focus();
    } else if (app.isReady()) {
      createWindow();
    }
  });

  app.on('ready', () => {
    // A packaged build takes its icon from the bundle; when run from source the Dock would show Electron's.
    if (process.platform === 'darwin' && !app.isPackaged) app.dock?.setIcon(path.join(app.getAppPath(), 'assets', 'icon.png'));
    initDb();
    registerHandlers(refreshTray);
    createWindow();
    createTray();
    if (settings.get().monitorEnabled) startMonitor(() => broadcast('activity'));
    startClaudeWatcher(() => broadcast('activity'));
  });
}

app.on('before-quit', () => {
  isQuitting = true;
});

app.on('will-quit', () => {
  stopMonitor();
  stopClaudeWatcher();
  destroyTray();
});

// The menu-bar timer keeps running with the window closed; Quit ends it.
app.on('window-all-closed', () => {
  if (process.platform === 'darwin') return;
  if ((process.platform === 'linux' || process.platform === 'win32') && hasTray() && !isQuitting) return;
  app.quit();
});

app.on('activate', () => {
  const win = BrowserWindow.getAllWindows()[0];
  if (win) {
    if (!win.isVisible()) win.show();
    win.focus();
  } else if (gotSingleInstanceLock) {
    createWindow();
  }
});
