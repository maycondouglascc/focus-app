import assert from 'node:assert/strict';
import test from 'node:test';

const { chooseAppName, exeStem, normalizeBrowserUrl, normalizeExeKey, parseWorkerLine } = (await import(
  new URL('./normalize.ts', import.meta.url).href
)) as typeof import('./normalize');

test('exeStem extracts executable basename without .exe suffix', () => {
  assert.equal(exeStem('C:\\Program Files\\Microsoft VS Code\\Code.exe'), 'Code');
  assert.equal(exeStem('C:\\Windows\\System32\\notepad.EXE'), 'notepad');
  assert.equal(exeStem('chrome.exe'), 'chrome');
  assert.equal(exeStem('/usr/bin/code'), 'code');
});

test('normalizeExeKey strips Squirrel version directories and normalizes Store packages', () => {
  assert.equal(
    normalizeExeKey('C:\\Users\\Maycon\\AppData\\Local\\Discord\\app-1.0.9165\\Discord.exe'),
    'c:\\users\\maycon\\appdata\\local\\discord\\discord.exe',
  );
  assert.equal(
    normalizeExeKey('C:\\Users\\Maycon\\AppData\\Local\\Discord\\app-1.0.9200\\Discord.exe'),
    'c:\\users\\maycon\\appdata\\local\\discord\\discord.exe',
  );
  assert.equal(
    normalizeExeKey('C:\\Program Files\\WindowsApps\\Microsoft.WindowsCalculator_11.2405.2.0_x64__8wekyb3d8bbwe\\CalculatorApp.exe'),
    'pkg:microsoft.windowscalculator_8wekyb3d8bbwe/calculatorapp',
  );
  assert.equal(
    normalizeExeKey('C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'),
    'c:\\program files\\google\\chrome\\application\\chrome.exe',
  );
});

test('chooseAppName prefers FileDescription, then ProductName, skipping generic values', () => {
  assert.equal(
    chooseAppName('Visual Studio Code', 'Code', 'C:\\Program Files\\Microsoft VS Code\\Code.exe'),
    'Visual Studio Code',
  );
  assert.equal(
    chooseAppName('Electron', 'Slack', 'C:\\Users\\me\\AppData\\Local\\slack\\app-4.38.0\\slack.exe'),
    'Slack',
  );
  assert.equal(
    chooseAppName('Java(TM) Platform SE binary', 'Microsoft® Windows® Operating System', 'C:\\Program Files\\WindowsApps\\wt\\WindowsTerminal.exe'),
    'Windows Terminal',
  );
  assert.equal(
    chooseAppName('Host Process for Windows Services', '', 'C:\\Tools\\MyCustomTool.exe'),
    'MyCustomTool',
  );
});

test('normalizeBrowserUrl cleans valid web URLs and rejects search queries or internal pages', () => {
  assert.equal(normalizeBrowserUrl('https://github.com/org/focus-app'), 'https://github.com/org/focus-app');
  assert.equal(normalizeBrowserUrl('github.com/org/focus-app'), 'https://github.com/org/focus-app');
  assert.equal(normalizeBrowserUrl('localhost:5174/settings'), 'http://localhost:5174/settings');
  assert.equal(normalizeBrowserUrl('127.0.0.1:8080/api'), 'http://127.0.0.1:8080/api');
  assert.equal(normalizeBrowserUrl('how to build electron app'), '');
  assert.equal(normalizeBrowserUrl('chrome://settings'), '');
  assert.equal(normalizeBrowserUrl('edge://newtab'), '');
  assert.equal(normalizeBrowserUrl('about:blank'), '');
  assert.equal(normalizeBrowserUrl('file:///C:/Users/me/index.html'), '');
  assert.equal(normalizeBrowserUrl(''), '');
});

test('parseWorkerLine parses valid JSONL messages and ignores noise', () => {
  assert.deepEqual(parseWorkerLine('{"ready":true,"uia":true,"audio":false}'), {
    ready: true,
    uia: true,
    audio: false,
  });
  assert.equal(parseWorkerLine(''), null);
  assert.equal(parseWorkerLine('WARNING: something'), null);
  assert.equal(parseWorkerLine('{broken json'), null);
});
