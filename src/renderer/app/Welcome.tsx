import { useEffect, useState, type CSSProperties } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { Settings } from '@/shared/types';
import { api, useMonitorStatus, useSaveSettings } from '@/renderer/lib/api';
import { browserTabsHint, describeAccessState, isMac, isWin, useNativeTitleBar } from '@/renderer/lib/platform';
import { Icon, type IconName } from '@/renderer/components/Icon';
import { LoButton } from '@/renderer/components/LoButton';

const DRAG = { WebkitAppRegion: 'drag' } as CSSProperties;
const NO_DRAG = { WebkitAppRegion: 'no-drag' } as CSSProperties;

type Source = { key: 'monitorEnabled' | 'browserTabs' | 'claudeSessions'; icon: IconName; title: string; hint: string };

const SOURCES: Source[] = [
  {
    key: 'monitorEnabled',
    icon: 'macwindow',
    title: 'Apps and windows',
    hint: 'The app in front and its window title, every 5 seconds. Pauses when you step away or lock the computer.',
  },
  {
    key: 'browserTabs',
    icon: 'globe',
    title: 'Browser tabs',
    hint: 'The title and address of the tab you are on in Arc, Chrome, Brave, Edge and Safari.',
  },
  {
    key: 'claudeSessions',
    icon: 'terminal',
    title: 'Claude Code sessions',
    hint: 'Which project and session you worked in, and when. Never the conversation itself.',
  },
];

/**
 * First launch: says what Focus records before it records anything, lets each source be switched off, and asks for
 * window-title access with the reason next to the button. Recording starts when the user clicks "Start using Focus".
 */
export function Welcome({ settings }: { settings: Settings }) {
  const save = useSaveSettings();
  const qc = useQueryClient();
  const status = useMonitorStatus().data;
  const [on, setOn] = useState({
    monitorEnabled: settings.monitorEnabled,
    browserTabs: settings.browserTabs,
    claudeSessions: settings.claudeSessions,
  });
  const [asked, setAsked] = useState(false);
  const access = describeAccessState(status, asked);
  const allowed = access.allowed;
  const browserTabsSupported = status?.capabilities?.browserTabs ?? (isMac || isWin);

  // Watch for permission / extension activation while the welcome screen is open.
  useEffect(() => {
    if ((!asked && status?.permissionState !== 'needs-relogin') || allowed) return;
    const t = setInterval(() => void qc.invalidateQueries({ queryKey: ['monitor'] }), 1500);
    return () => clearInterval(t);
  }, [asked, allowed, status?.permissionState, qc]);

  const titlesNeeded = on.monitorEnabled;

  return (
    <div className="fo-welcome" style={useNativeTitleBar ? undefined : DRAG}>
      <div className="fo-welcome-card" style={useNativeTitleBar ? undefined : NO_DRAG}>
        <div className="fo-welcome-brand">Focus</div>
        <h1 className="fo-welcome-title">See where your time actually goes</h1>
        <p className="fo-welcome-lead">
          Log time against your projects, and let Focus quietly keep track of what you worked on, so nothing you did slips
          through. Here is what it records. Change any of it later in Settings.
        </p>

        <ul className="fo-welcome-sources">
          {SOURCES.map((s) => {
            const unsupported = s.key === 'browserTabs' && !browserTabsSupported;
            const disabled = unsupported || (s.key === 'browserTabs' && !on.monitorEnabled);
            const hint = s.key === 'browserTabs' ? browserTabsHint(status?.platform, browserTabsSupported) : s.hint;
            return (
              <li key={s.key}>
                <label className="fo-welcome-source" data-disabled={disabled}>
                  <span className="fo-welcome-icon">
                    <Icon name={s.icon} size={20} />
                  </span>
                  <span className="fo-welcome-text">
                    <span className="lo-title">{s.title}</span>
                    <span className="lo-meta">{hint}</span>
                  </span>
                  <span className="fo-switch">
                    <input
                      type="checkbox"
                      checked={on[s.key] && !disabled}
                      disabled={disabled}
                      onChange={(e) => setOn({ ...on, [s.key]: e.target.checked })}
                      aria-label={s.title}
                    />
                    <span className="fo-switch-track" aria-hidden="true" />
                  </span>
                </label>
              </li>
            );
          })}
        </ul>

        {titlesNeeded && (
          <div className="fo-welcome-access" data-allowed={allowed}>
            <span className="fo-welcome-text">
              <span className="lo-title">{access.title}</span>
              <span className="lo-meta">{access.hint}</span>
            </span>
            {allowed ? (
              <span className="fo-ok">
                <Icon name="checkmark" size={14} /> Allowed
              </span>
            ) : (
              access.actionLabel && (
                <LoButton
                  small
                  onClick={() => {
                    setAsked(true);
                    void api.monitor.requestAccess().then(() => qc.invalidateQueries({ queryKey: ['monitor'] }));
                  }}
                >
                  {access.actionLabel}
                </LoButton>
              )
            )}
          </div>
        )}

        <p className="fo-welcome-privacy">
          <Icon name="lock" size={14} />
          Everything stays in one file on this computer. No account, no cloud, no analytics.
        </p>

        <div className="fo-welcome-foot">
          <LoButton variant="primary" onClick={() => save.mutate({ ...on, onboarded: true })} disabled={save.isPending}>
            Start using Focus
          </LoButton>
        </div>
      </div>
    </div>
  );
}
