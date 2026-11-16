import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { rebuild } from '@electron/rebuild';
import type { ForgeConfig } from '@electron-forge/shared-types';
import { MakerZIP } from '@electron-forge/maker-zip';
import { VitePlugin } from '@electron-forge/plugin-vite';
import { AutoUnpackNativesPlugin } from '@electron-forge/plugin-auto-unpack-natives';
import { FusesPlugin } from '@electron-forge/plugin-fuses';
import { FuseV1Options, FuseVersion } from '@electron/fuses';

const config: ForgeConfig = {
  packagerConfig: {
    asar: true,
    name: 'Focus',
    // App icon (.icns is added by the packager). Source artwork: assets/Icon-iOS-Default-1024@2x.png, padded to the macOS grid.
    icon: './assets/icon',
    // The menu-bar icon is loaded from disk at runtime (src/main/tray.ts).
    extraResource: ['./assets'],
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
  // macOS only (SF Symbols, Terminal sign-in, hiddenInset title bar).
  makers: [new MakerZIP({}, ['darwin'])],
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
      [FuseV1Options.EnableCookieEncryption]: true,
      [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
      [FuseV1Options.EnableNodeCliInspectArguments]: false,
      [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
      [FuseV1Options.OnlyLoadAppFromAsar]: true,
    }),
  ],
};

export default config;
