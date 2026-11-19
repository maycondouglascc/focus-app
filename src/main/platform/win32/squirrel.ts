import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

/**
 * Handles Squirrel.Windows installer lifecycle arguments (`--squirrel-install`,
 * `--squirrel-updated`, `--squirrel-uninstall`, `--squirrel-obsolete`).
 * Returns `true` when the app should exit immediately without starting the UI.
 */
export function handleSquirrelEvent(argv: string[] = process.argv, execPath: string = process.execPath): boolean {
  if (process.platform !== 'win32') return false;
  const cmd = argv[1];
  if (!cmd || !cmd.startsWith('--squirrel-')) return false;

  const exeName = path.win32.basename(execPath);
  const appFolder = path.win32.dirname(execPath);
  const rootFolder = path.win32.dirname(appFolder);
  const updateExe = path.win32.join(rootFolder, 'Update.exe');

  const runUpdate = (args: string[]) => {
    if (!fs.existsSync(updateExe)) return;
    try {
      spawnSync(updateExe, args, { windowsHide: true, timeout: 15_000 });
    } catch {
      /* ignore shortcut failures */
    }
  };

  switch (cmd) {
    case '--squirrel-install':
    case '--squirrel-updated':
      runUpdate(['--createShortcut', exeName]);
      return true;
    case '--squirrel-uninstall':
      runUpdate(['--removeShortcut', exeName]);
      return true;
    case '--squirrel-obsolete':
      return true;
    case '--squirrel-firstrun':
    default:
      return false;
  }
}
