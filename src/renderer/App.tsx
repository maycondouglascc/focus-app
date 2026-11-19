import { useEffect, useState, type CSSProperties } from 'react';
import { Reshaped } from 'reshaped';
import { QueryClientProvider } from '@tanstack/react-query';
import { queryClient, useLiveData, useSettings } from './lib/api';
import { isMac, useNativeTitleBar } from './lib/platform';
import { Welcome } from './app/Welcome';
import { DataErrorBanner, ScreenBoundary } from './components/ErrorStates';
import { Sidebar, type Section } from './app/Sidebar';
import { CalendarScreen } from './features/calendar/CalendarScreen';
import { ReportsScreen } from './features/reports/ReportsScreen';
import { ProjectsScreen } from './features/projects/ProjectsScreen';
import { SettingsScreen } from './features/settings/SettingsScreen';
import { ActivityScreen } from './features/activity/ActivityScreen';
import { ProjectReportScreen } from './features/projects/ProjectReportScreen';

const DRAG = { WebkitAppRegion: 'drag' } as CSSProperties;

const KEYS: Record<string, Section> = { '1': 'calendar', '2': 'activity', '3': 'reports', '4': 'projects', ',': 'settings' };

function Shell() {
  useLiveData();
  const settings = useSettings();
  const [section, setSection] = useState<Section>('calendar');
  const [projectId, setProjectId] = useState<string | null>(null);
  const openProject = (id: string) => {
    setProjectId(id);
    setSection('project');
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
      if (!mod || e.shiftKey || e.altKey) return;
      const s = KEYS[e.key];
      if (s) {
        e.preventDefault();
        setSection(s);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // First launch: the welcome comes before anything is recorded or shown.
  if (settings.data && !settings.data.onboarded) return <Welcome settings={settings.data} />;

  return (
    <div className="shell-root">
      <Sidebar section={section} projectId={projectId} onSelect={setSection} onProject={openProject} />
      <main className="shell-card">
        {!useNativeTitleBar && <div className="shell-dragstrip" style={DRAG} aria-hidden="true" />}
        <div className="shell-content">
          <DataErrorBanner />
          <ScreenBoundary key={section + (projectId ?? '')}>
          {section === 'calendar' && <CalendarScreen onNewProject={() => setSection('projects')} />}
          {section === 'activity' && <ActivityScreen />}
          {section === 'reports' && <ReportsScreen onTimer={() => setSection('calendar')} />}
          {section === 'projects' && <ProjectsScreen onOpen={openProject} />}
          {section === 'project' && projectId && <ProjectReportScreen key={projectId} projectId={projectId} onBack={() => setSection('projects')} />}
          {section === 'settings' && <SettingsScreen />}
          </ScreenBoundary>
        </div>
      </main>
    </div>
  );
}

export default function App() {
  return (
    <Reshaped theme="slate" defaultColorMode="light">
      <QueryClientProvider client={queryClient}>
        <Shell />
      </QueryClientProvider>
    </Reshaped>
  );
}
