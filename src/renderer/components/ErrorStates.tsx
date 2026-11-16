import { Component, useSyncExternalStore, type ReactNode } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { Icon } from '@/renderer/components/Icon';
import { LoButton } from '@/renderer/components/LoButton';

/** A screen that throws while rendering shows this instead of going blank. Keyed by screen, so navigating away resets it. */
export class ScreenBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error) {
    console.error('[focus] screen crashed', error);
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="fo-crash" role="alert">
        <Icon name="exclamationmark.triangle" size={28} />
        <h2 className="lo-h2">This screen ran into a problem</h2>
        <p className="lo-sub">Your data is safe. Try again, or switch to another screen and back.</p>
        <p className="fo-crash-detail">{this.state.error.message}</p>
        <LoButton icon="arrow.clockwise" onClick={() => this.setState({ error: null })}>
          Try again
        </LoButton>
      </div>
    );
  }
}

/** Shown above the screen when reading from the local database fails; retries the failed reads. */
export function DataErrorBanner() {
  const qc = useQueryClient();
  const cache = qc.getQueryCache();
  const failed = useSyncExternalStore(
    (cb) => cache.subscribe(cb),
    () => cache.getAll().some((q) => q.state.status === 'error' && q.state.fetchStatus === 'idle'),
  );
  if (!failed) return null;
  return (
    <div className="fo-dataerror" role="alert">
      <Icon name="exclamationmark.triangle" size={16} />
      <span>Focus couldn't read some of its data, so parts of this screen may be missing.</span>
      <LoButton small onClick={() => void qc.refetchQueries({ predicate: (q) => q.state.status === 'error' })}>
        Try again
      </LoButton>
    </div>
  );
}
