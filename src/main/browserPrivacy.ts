export const APP_TITLE_ALIASES: Record<string, string[]> = {
  firefox: ['Mozilla Firefox', 'Firefox Developer Edition', 'Firefox Nightly'],
  'firefox web browser': ['Mozilla Firefox', 'Firefox'],
  'google chrome': ['Google Chrome', 'Chrome'],
  'brave browser': ['Brave'],
  'brave web browser': ['Brave', 'Brave Browser'],
  'microsoft edge': ['Microsoft Edge', 'Edge'],
  'zen browser': ['Zen Browser', 'Zen'],
  'chromium web browser': ['Chromium'],
};

const BROWSER_NAMES = new Set([
  'firefox',
  'mozilla firefox',
  'firefox developer edition',
  'firefox nightly',
  'firefox web browser',
  'librewolf',
  'zen',
  'zen browser',
  'floorp',
  'waterfox',
  'tor browser',
  'mullvad browser',
  'pale moon',
  'palemoon',
  'icecat',
  'gnu icecat',
  'seamonkey',
  'google chrome',
  'chrome',
  'google chrome canary',
  'google chrome beta',
  'google chrome dev',
  'google chrome unstable',
  'chromium',
  'chromium browser',
  'chromium web browser',
  'ungoogled chromium',
  'thorium',
  'brave',
  'brave browser',
  'brave web browser',
  'microsoft edge',
  'edge',
  'microsoft edge dev',
  'microsoft edge beta',
  'vivaldi',
  'opera',
  'opera gx',
  'opera beta',
  'opera developer',
  'arc',
  'safari',
  'safari technology preview',
  'epiphany',
  'gnome web',
  'falkon',
  'midori',
  'qutebrowser',
  'orion',
  'duckduckgo',
  'yandex',
  'yandex browser',
  'comet',
  'dia',
  'ladybird',
]);

const BROWSER_BUNDLE_TOKENS = new Set([
  'firefox',
  'firefox-esr',
  'firefox-developer-edition',
  'firefox-nightly',
  'librewolf',
  'librewolf-community',
  'zen',
  'zen-browser',
  'zen_browser',
  'floorp',
  'waterfox',
  'torbrowser',
  'tor-browser',
  'mullvadbrowser',
  'mullvad-browser',
  'palemoon',
  'icecat',
  'seamonkey',
  'chrome',
  'google-chrome',
  'google-chrome-beta',
  'google-chrome-unstable',
  'chromium',
  'chromium-browser',
  'thorium',
  'brave',
  'brave-browser',
  'msedge',
  'microsoft-edge',
  'microsoft-edge-dev',
  'microsoft-edge-beta',
  'edgemac',
  'vivaldi',
  'vivaldi-stable',
  'opera',
  'arc',
  'comet',
  'dia',
  'duckduckgo',
  'safari',
  'safaritechnologypreview',
  'epiphany',
  'falkon',
  'midori',
  'qutebrowser',
  'kagimacos',
  'yandex',
  'yandex-browser',
]);

const BROWSER_BUNDLE_EXACT = new Set([
  'company.thebrowser.browser',
  'com.duckduckgo.macos.browser',
  'org.mozilla.firefox',
  'org.mozilla.firefoxdeveloperedition',
  'org.mozilla.nightly',
  'com.google.chrome',
  'com.google.chrome.canary',
  'org.chromium.chromium',
  'com.brave.browser',
  'com.microsoft.edgemac',
  'com.vivaldi.vivaldi',
  'com.operasoftware.opera',
  'com.apple.safari',
  'com.apple.safaritechnologypreview',
  'org.gnome.epiphany',
  'org.kde.falkon',
  'org.qutebrowser.qutebrowser',
  'app.zen_browser.zen',
  'app.zen-browser.zen',
  'io.github.zen_browser.zen',
  'io.gitlab.librewolf-community',
  'one.ablaze.floorp',
  'net.waterfox.waterfox',
  'com.kagi.kagimacos',
]);

const EXTRA_BROWSER_SUFFIXES = [
  'Google Chrome',
  'Chrome',
  'Chromium',
  'Microsoft Edge',
  'Edge',
  'Brave',
  'Brave Browser',
  'Mozilla Firefox',
  'Firefox',
  'Firefox Developer Edition',
  'Firefox Nightly',
  'Zen Browser',
  'Zen',
  'LibreWolf',
  'Floorp',
  'Waterfox',
  'Tor Browser',
  'Mullvad Browser',
  'Vivaldi',
  'Opera',
  'Safari',
  'Web',
  'Epiphany',
  'Falkon',
  'Midori',
  'Orion',
  'Arc',
  'DuckDuckGo',
  'qutebrowser',
];

const BRACKETED_PRIVATE_SUFFIX =
  /(?:\(|\[)\s*(?:incognito|inprivate|private(?:\s+browsing|\s+window|\s+tab)?|an[oô]nim[ao]|navega[cç][aã]o\s+privad[ao]|navega[cç][aã]o\s+privativa|navegaci[oó]n\s+privada|de\s+inc[oó]gnito|inc[oó]gnito|navigation\s+priv[eé]e|privater\s+modus|inkognito|navigazione\s+(?:anonima|in\s+incognito)|priv[eé]navigatie|tryb\s+prywatny|приватный\s+просмотр)\s*(?:\)|\])$/i;

const TRAILING_PRIVATE_PHRASE =
  /(?:^|[\s—–-])(?:private\s+browsing|private\s+window|private\s+with\s+tor|navega[cç][aã]o\s+privativa(?:\s+d[eo]\s+[\p{L}\p{N}\s._-]+)?|navega[cç][aã]o\s+privada(?:\s+d[eo]\s+[\p{L}\p{N}\s._-]+)?|navegaci[oó]n\s+privada(?:\s+de\s+[\p{L}\p{N}\s._-]+)?|navigation\s+priv[eé]e(?:\s+de\s+[\p{L}\p{N}\s._-]+)?|privater\s+modus(?:\s+von\s+[\p{L}\p{N}\s._-]+)?|navigazione\s+(?:anonima|in\s+incognito)|priv[eé]navigatie|tryb\s+prywatny|приватный\s+просмотр|プライベートブラウジング|隐私浏览|隱私瀏覽|无痕浏览|無痕瀏覽|사생활\s+보호\s+모드|시크릿\s+모드)$/iu;

const SEPARATED_INCOGNITO_TOKEN = /(?:^|\s[—–-]\s*)(?:incognito|inprivate)$/i;

const PRIVATE_NEW_TAB_TITLES = new Set([
  'new incognito tab',
  'new inprivate tab',
  'new private tab',
  'new private window',
  'new private window with tor',
  'nova guia anônima',
  'nova aba anônima',
  'guia anônima',
  'nova guia inprivate',
  'nova aba inprivate',
  'nova aba privada',
  'nova janela privada',
  'janela privada',
  'janela privativa',
  'nueva pestaña de incógnito',
  'nueva pestaña inprivate',
  'nueva pestaña privada',
  'nouvel onglet de navigation privée',
  'nouvel onglet inprivate',
  'nouvel onglet privé',
  'neuer inkognito-tab',
  'neuer inprivate-tab',
  'neuer privater tab',
  'nuova scheda di navigazione in incognito',
  'nuova scheda inprivate',
  'вкладка инкогнито',
]);

function escapeRegex(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function isBrowserApp(app: string, bundleId = ''): boolean {
  const cleanApp = app.trim().toLowerCase();
  if (BROWSER_NAMES.has(cleanApp)) return true;

  const cleanBid = bundleId.trim().toLowerCase().replace(/\.desktop$/, '');
  if (!cleanBid) return false;
  if (BROWSER_BUNDLE_EXACT.has(cleanBid)) return true;

  // Windows bundleIds are normalized executable paths (e.g. `c:\...\chrome.exe`) or `pkg:.../msedge`.
  if (cleanBid.includes('\\') || cleanBid.includes('/')) {
    const exeName = cleanBid.split(/[\\/]/).pop()?.replace(/\.exe$/, '') || '';
    if (exeName && BROWSER_BUNDLE_TOKENS.has(exeName)) return true;
  }

  const parts = cleanBid.split(/[._]/).filter(Boolean);
  const tail = parts[parts.length - 1];
  const head = parts[0];
  return BROWSER_BUNDLE_TOKENS.has(cleanBid) || (!!tail && BROWSER_BUNDLE_TOKENS.has(tail)) || (!!head && BROWSER_BUNDLE_TOKENS.has(head));
}

function stripTrailingAppSuffix(title: string, app: string): string {
  const names = [app, ...(APP_TITLE_ALIASES[app.toLowerCase()] ?? []), ...EXTRA_BROWSER_SUFFIXES].filter(Boolean);
  for (const name of names) {
    const suffix = new RegExp(`\\s[—–-]\\s${escapeRegex(name)}$`, 'i');
    if (suffix.test(title)) {
      return title.replace(suffix, '').trim();
    }
  }
  return title;
}

function matchesPrivateTitle(candidate: string): boolean {
  const clean = candidate.trim().replace(/\s+/g, ' ');
  if (!clean) return false;
  if (PRIVATE_NEW_TAB_TITLES.has(clean.toLowerCase())) return true;
  if (BRACKETED_PRIVATE_SUFFIX.test(clean)) return true;
  if (TRAILING_PRIVATE_PHRASE.test(clean)) return true;
  if (SEPARATED_INCOGNITO_TOKEN.test(clean)) return true;
  return false;
}

export function isPrivateBrowserWindow(sample: {
  app: string;
  bundleId?: string;
  title?: string;
  tab?: string;
  isPrivate?: boolean;
}): boolean {
  if (sample.isPrivate) return true;
  if (!isBrowserApp(sample.app, sample.bundleId ?? '')) return false;

  const candidates = new Set<string>();
  for (const raw of [sample.title, sample.tab]) {
    if (!raw) continue;
    const trimmed = raw.trim();
    if (!trimmed) continue;
    candidates.add(trimmed);
    candidates.add(stripTrailingAppSuffix(trimmed, sample.app));
  }

  for (const c of candidates) {
    if (matchesPrivateTitle(c)) return true;
  }
  return false;
}
