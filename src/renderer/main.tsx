import './lib/demoClock'; // first: in demo mode every later module must see the shifted clock
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import 'reshaped/themes/slate/theme.css';
import 'reshaped/bundle.css';
import '@fontsource-variable/newsreader/opsz.css';
import './styles/index.css';
import App from './App';

const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root element.');

// Inside the Mac app the window itself is translucent (vibrancy); the page must not paint over it.
if (window.focusApp && navigator.userAgent.includes('Macintosh')) document.documentElement.dataset.vibrancy = '';

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
