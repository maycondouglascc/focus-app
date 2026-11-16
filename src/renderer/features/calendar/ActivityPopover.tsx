import { useEffect, useState } from 'react';
import { Popover } from '@/renderer/components/Popover';
import { LoButton } from '@/renderer/components/LoButton';
import { appColor } from '@/renderer/lib/colors';
import { useAppIcons } from '@/renderer/lib/api';
import { AppIcon } from '@/renderer/components/AppIcon';
import { duration, time } from '@/renderer/lib/format';
import { sessionLabel, type ActivitySession } from './layout';
import { Icon } from '@/renderer/components/Icon';

/** Under a minute reads as "<1m", not "0m". */
const len = (ms: number) => (ms < 60_000 ? '<1m' : duration(ms));

/** One session at the computer: which apps, what inside them, for how long, and a way to log it as time. */
export function ActivityPopover({
  session,
  anchor,
  onClose,
  onLog,
  closing,
}: {
  session: ActivitySession;
  anchor: DOMRect;
  onClose: () => void;
  onLog: (s: ActivitySession, description: string) => void;
  closing?: boolean;
}) {
  const icons = useAppIcons(session.apps.map((a) => a.name));
  const span = session.end - session.start;
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const toggle = (name: string) =>
    setOpen((cur) => {
      const next = new Set(cur);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  // Another session: start folded again.
  useEffect(() => setOpen(new Set()), [session.key]);
  const max = Math.max(...session.apps.map((x) => x.ms));
  return (
    <Popover anchor={anchor} onClose={onClose} closing={closing} label="Activity session" width={360}>
      <div className="fo-actpop">
        <div className="fo-actpop-head">
          <div>
            <div className="fo-actpop-app">
              {time(session.start)} – {time(session.end)}
            </div>
            <div className="lo-meta">
              {duration(session.activeMs)} active
              {span - session.activeMs >= 60_000 ? ` · ${duration(span - session.activeMs)} of short breaks` : ''}
            </div>
          </div>
        </div>
        <ul className="fo-actpop-apps">
          {session.apps.map((a) => {
            // Windows stay folded until asked for.
            const many = a.details.length > 0;
            const expanded = open.has(a.name);
            const head = (
              <>
                <span className="fo-actpop-approw">
                  <AppIcon name={a.name} src={icons[a.name]} size={18} />
                  <span className="fo-actpop-appname">
                    {a.name}
                    {many && <small>{a.details.length}</small>}
                  </span>
                  <span className="fo-actpop-len">{len(a.ms)}</span>
                  {many && <Icon name="chevron.right" className="fo-actpop-chev" aria-hidden="true" />}
                </span>
                <span className="fo-actpop-bar">
                  <span style={{ width: `${Math.max(1, (a.ms / max) * 100)}%`, background: appColor(a.name) }} />
                </span>
              </>
            );
            return (
              <li key={a.name} data-open={expanded}>
                {many ? (
                  <button type="button" className="fo-actpop-apphead" aria-expanded={expanded} onClick={() => toggle(a.name)}>
                    {head}
                  </button>
                ) : (
                  <div className="fo-actpop-apphead">{head}</div>
                )}
                {many && (
                  <div className="lo-reveal" data-open={expanded} aria-hidden={!expanded}>
                  <div className="lo-reveal-inner">
                  <ul className="fo-actpop-list">
                    {a.details.map((d) => (
                      <li key={d.label + d.sub}>
                        <div className="fo-actpop-text">
                          <span>{d.label}</span>
                          {d.sub && <small>{d.sub}</small>}
                        </div>
                        <span className="fo-actpop-len">{len(d.ms)}</span>
                      </li>
                    ))}
                  </ul>
                  </div>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
        <div className="fo-actpop-foot">
          <LoButton small icon="plus" onClick={() => onLog(session, sessionLabel(session))}>
            Log as time entry
          </LoButton>
        </div>
      </div>
    </Popover>
  );
}
