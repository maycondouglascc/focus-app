import assert from 'node:assert/strict';
import test from 'node:test';

const { isBrowserApp, isPrivateBrowserWindow } = (await import(
  new URL('./browserPrivacy.ts', import.meta.url).href
)) as typeof import('./browserPrivacy');

test('recognizes browsers by app name or bundleId and ignores non-browsers', () => {
  assert.equal(isBrowserApp('Google Chrome', 'com.google.Chrome'), true);
  assert.equal(isBrowserApp('Firefox Web Browser', 'firefox_firefox'), true);
  assert.equal(isBrowserApp('Zen Browser', 'app.zen_browser.zen'), true);
  assert.equal(isBrowserApp('Web', 'org.gnome.Epiphany'), true);
  assert.equal(isBrowserApp('Custom Chromium', 'c:\\program files\\google\\chrome\\application\\chrome.exe'), true);
  assert.equal(isBrowserApp('Edge Browser', 'c:\\program files (x86)\\microsoft\\edge\\application\\msedge.exe'), true);
  assert.equal(isBrowserApp('Visual Studio Code', 'c:\\users\\me\\appdata\\local\\programs\\microsoft vs code\\code.exe'), false);
  assert.equal(isBrowserApp('Visual Studio Code', 'code'), false);
  assert.equal(isBrowserApp('Web', 'com.example.web'), false);
});

test('trusts explicit platform isPrivate flag', () => {
  assert.equal(
    isPrivateBrowserWindow({
      app: 'Google Chrome',
      bundleId: 'com.google.Chrome',
      title: 'Example Domain',
      isPrivate: true,
    }),
    true,
  );
});

test('detects private browser windows across browsers and locales', () => {
  const privateSamples = [
    { app: 'Firefox Web Browser', bundleId: 'firefox_firefox', title: 'Docs — Mozilla Firefox Private Browsing' },
    { app: 'Firefox', bundleId: 'org.mozilla.firefox', title: 'Mozilla Firefox Private Browsing' },
    { app: 'Firefox', bundleId: 'firefox', title: 'Private Browsing' },
    { app: 'Firefox', bundleId: 'firefox', title: 'Docs — Navegação privativa do Mozilla Firefox' },
    { app: 'Firefox', bundleId: 'firefox', title: 'Docs — Mozilla Firefox Navegação privativa' },
    { app: 'Firefox', bundleId: 'firefox', title: 'Docs — Mozilla Firefox — Navegação privativa' },
    { app: 'Firefox', bundleId: 'firefox', title: 'Docs — Mozilla Firefox (Privater Modus)' },
    { app: 'Firefox', bundleId: 'firefox', title: 'Docs — Navigation privée' },
    { app: 'Zen Browser', bundleId: 'app.zen_browser.zen', title: 'Docs — Zen Browser Private Browsing' },
    { app: 'LibreWolf', bundleId: 'io.gitlab.librewolf-community', title: 'LibreWolf Private Browsing' },
    { app: 'Google Chrome', bundleId: 'google-chrome', title: 'Example - Google Chrome (Incognito)' },
    { app: 'Google Chrome', bundleId: 'google-chrome', title: 'New Incognito Tab - Google Chrome' },
    { app: 'Google Chrome', bundleId: 'google-chrome', title: 'Nova guia anônima - Google Chrome' },
    { app: 'Chromium', bundleId: 'chromium', title: 'Example - Chromium (Incognito)' },
    { app: 'Microsoft Edge', bundleId: 'microsoft-edge', title: 'New InPrivate tab - Microsoft Edge' },
    { app: 'Microsoft Edge', bundleId: 'microsoft-edge', title: 'Nova guia InPrivate - Microsoft Edge' },
    { app: 'Microsoft Edge', bundleId: 'microsoft-edge', title: 'Example [InPrivate] - Microsoft Edge' },
    { app: 'Brave Browser', bundleId: 'brave-browser', title: 'New Private Tab - Brave' },
    { app: 'Brave Browser', bundleId: 'brave-browser', title: 'New Private Window with Tor - Brave' },
    { app: 'Vivaldi', bundleId: 'vivaldi-stable', title: 'Example - Vivaldi (Private)' },
    { app: 'Safari', bundleId: 'com.apple.Safari', title: 'Private Browsing' },
  ];

  for (const sample of privateSamples) {
    assert.equal(isPrivateBrowserWindow(sample), true, `expected private for: ${sample.title}`);
  }
});

test('does not ignore normal browser windows or non-browser windows with matching words', () => {
  const normalSamples = [
    { app: 'Google Chrome', bundleId: 'google-chrome', title: 'What is Incognito - Google Chrome' },
    { app: 'Google Chrome', bundleId: 'google-chrome', title: 'Roadmap - Private - Google Chrome' },
    { app: 'Firefox Web Browser', bundleId: 'firefox_firefox', title: 'Private Browsing with Firefox | Firefox Help — Mozilla Firefox' },
    { app: 'Microsoft Edge', bundleId: 'microsoft-edge', title: 'Browse InPrivate in Microsoft Edge - Microsoft Edge' },
    { app: 'Visual Studio Code', bundleId: 'code', title: 'Private Browsing - focus-app - Visual Studio Code' },
    { app: 'Code', bundleId: 'com.microsoft.VSCode', title: 'incognito.ts — focus-app' },
  ];

  for (const sample of normalSamples) {
    assert.equal(isPrivateBrowserWindow(sample), false, `expected tracked for: ${sample.title}`);
  }
});
