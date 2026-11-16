import { useEffect, useState, type CSSProperties } from 'react';
import { Reshaped } from 'reshaped';
import { QueryClientProvider } from '@tanstack/react-query';
import { queryClient, useLiveData } from './lib/api';
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
  const [section, setSection] = useState<Section>('calendar');
  const [projectId, setProjectId] = useState<string | null>(null);
  const openProject = (id: string) => {
    setProjectId(id);
    setSection('project');
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.metaKey || e.shiftKey || e.altKey || e.ctrlKey) return;
      const s = KEYS[e.key];
      if (s) {
        e.preventDefault();
        setSection(s);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div className="shell-root">
      <Sidebar section={section} projectId={projectId} onSelect={setSection} onProject={openProject} />
      <main className="shell-card">
        <div className="shell-dragstrip" style={DRAG} aria-hidden="true" />
        <div className="shell-content">
          {section === 'calendar' && <CalendarScreen />}
          {section === 'activity' && <ActivityScreen />}
          {section === 'reports' && <ReportsScreen />}
          {section === 'projects' && <ProjectsScreen onOpen={openProject} />}
          {section === 'project' && projectId && <ProjectReportScreen key={projectId} projectId={projectId} onBack={() => setSection('projects')} />}
          {section === 'settings' && <SettingsScreen />}
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
