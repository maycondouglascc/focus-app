import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { rebuild } from '@electron/rebuild';
import type { ForgeConfig } from '@electron-forge/shared-types';
import { MakerZIP } from '@electron-forge/maker-zip';
import { MakerDeb } from '@electron-forge/maker-deb';
import { MakerRpm } from '@electron-forge/maker-rpm';
import { MakerSquirrel } from '@electron-forge/maker-squirrel';
import { VitePlugin } from '@electron-forge/plugin-vite';
import { AutoUnpackNativesPlugin } from '@electron-forge/plugin-auto-unpack-natives';
import { FusesPlugin } from '@electron-forge/plugin-fuses';
import { FuseV1Options, FuseVersion } from '@electron/fuses';

const hasCommand = (cmd: string) =>
  (process.env.PATH || '').split(path.delimiter).some((dir) => dir && fs.existsSync(path.join(dir, cmd)));

const linuxDesktop = {
  name: 'focus',
  productName: 'Focus',
  genericName: 'Time Tracker',
  description: 'Local time tracker with an activity stream',
  productDescription: 'Focus puts logged time entries and automatic activity side by side in a local-first desktop app.',
  icon: './assets/icon.png',
  categories: ['Utility', 'Office'] as ('Utility' | 'Office')[],
};

const config: ForgeConfig = {
  packagerConfig: {
    asar: true,
    name: 'Focus',
    ...(process.platform === 'linux' ? { executableName: 'focus' } : {}),
    // App icon (.icns on macOS, .ico on Windows, .png on Linux).
    icon: './assets/icon',
    win32metadata: {
      CompanyName: 'Focus',
      FileDescription: 'Focus — Local time tracker with an activity stream',
      OriginalFilename: 'Focus.exe',
      ProductName: 'Focus',
      InternalName: 'Focus',
    },
    // Tray icons, Linux GNOME extension, and Windows helper script are loaded from disk at runtime.
    extraResource: ['./assets', './resources/gnome-extension', './resources/win32'],
  },
  rebuildConfig: {},
  hooks: {
    // The Vite plugin bundles JS only and leaves native modules out of the package (better-sqlite3 is `external`).
    // Compile better-sqlite3 for this Electron (a fresh `npm install` builds it for Node), then copy it and its
    // runtime dependencies into the app.
    packageAfterCopy: async (_config, buildPath, electronVersion, _platform, arch) => {
      await rebuild({ buildPath: __dirname, electronVersion, arch, onlyModules: ['better-sqlite3'], force: true });
      const copy = (from: string) =>
        fs.cpSync(path.join(__dirname, 'node_modules', from), path.join(buildPath, 'node_modules', from), { recursive: true });
      for (const f of ['package.json', 'LICENSE', 'lib', 'build/Release/better_sqlite3.node']) copy(`better-sqlite3/${f}`);
      copy('bindings');
      copy('file-uri-to-path');
    },
    // Packaging edits Info.plist and flips fuses after Electron's own ad-hoc signature, which leaves it invalid: a
    // downloaded copy then reads as "damaged". Re-sign the finished bundle ad hoc so macOS sees a consistent app.
    // (No Developer ID yet, so it is still unnotarized: first launch needs Privacy & Security → Open Anyway.)
    postPackage: async (_config, { outputPaths, platform }) => {
      if (platform !== 'darwin') return;
      for (const dir of outputPaths) {
        const app = path.join(dir, 'Focus.app');
        execFileSync('codesign', ['--force', '--deep', '--sign', '-', app]);
        execFileSync('codesign', ['--verify', '--deep', '--strict', app]);
      }
    },
  },
  makers: [
    new MakerSquirrel(
      {
        name: 'focus',
        authors: 'Focus',
        description: 'Local time tracker with an activity stream',
        setupIcon: path.resolve(__dirname, 'assets', 'icon.ico'),
        setupExe: 'Focus-Setup.exe',
        noMsi: true,
      },
      ['win32'],
    ),
    new MakerZIP({}, ['darwin', 'linux', 'win32']),
    new MakerDeb({ options: linuxDesktop }, ['linux']),
    ...(hasCommand('rpmbuild') ? [new MakerRpm({ options: linuxDesktop }, ['linux'])] : []),
  ],
  plugins: [
    // Unpack native modules (better-sqlite3) from the asar so their .node binaries load.
    new AutoUnpackNativesPlugin({}),
    new VitePlugin({
      // `build` can specify multiple entry builds, which can be Main process, Preload scripts, Worker process, etc.
      // If you are familiar with Vite configuration, it will look really familiar.
      build: [
        {
          // `entry` is just an alias for `build.lib.entry` in the corresponding file of `config`.
          entry: 'src/main.ts',
          config: 'vite.main.config.ts',
          target: 'main',
        },
        {
          entry: 'src/preload.ts',
          config: 'vite.preload.config.ts',
          target: 'preload',
        },
      ],
      renderer: [
        {
          name: 'main_window',
          config: 'vite.renderer.config.mts',
        },
      ],
    }),
    // Fuses are used to enable/disable various Electron functionality
    // at package time, before code signing the application
    new FusesPlugin({
      version: FuseVersion.V1,
      [FuseV1Options.RunAsNode]: false,
      // Off: Focus has no logins or cookies worth encrypting, and the key would live in the Keychain, which asks for
      // the user's password whenever a new (ad-hoc signed) build reads it.
      [FuseV1Options.EnableCookieEncryption]: false,
      [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
      [FuseV1Options.EnableNodeCliInspectArguments]: false,
      [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
      [FuseV1Options.OnlyLoadAppFromAsar]: true,
    }),
  ],
};

export default config;
