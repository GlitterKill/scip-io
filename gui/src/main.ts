import { store, Screen } from './state/store.js';
import { renderTitlebar } from './components/Titlebar.js';
import { renderDashboard } from './components/Dashboard.js';
import { renderIndexProgress } from './components/IndexProgress.js';
import { renderResults } from './components/Results.js';
import { renderSettings } from './components/Settings.js';

function render(container: HTMLElement) {
  container.innerHTML = '';
  container.className = 'app-container';

  // Custom titlebar
  container.appendChild(renderTitlebar());

  // Main content area
  const main = document.createElement('div');
  main.className = 'main-content';
  container.appendChild(main);

  // Render current screen
  const renderScreen = (screen: Screen) => {
    main.innerHTML = '';
    main.className = 'main-content animate-fade-in';

    switch (screen) {
      case 'dashboard':
        renderDashboard(main);
        break;
      case 'indexing':
        renderIndexProgress(main);
        break;
      case 'results':
        renderResults(main);
        break;
      case 'settings':
        renderSettings(main);
        break;
    }
  };

  // Subscribe to screen changes
  let currentScreen = store.getState().screen;
  store.subscribe((state) => {
    if (state.screen !== currentScreen) {
      currentScreen = state.screen;
      renderScreen(state.screen);
    }
  });

  // Initial render
  renderScreen(store.getState().screen);
}

document.addEventListener('DOMContentLoaded', () => {
  const app = document.getElementById('app');
  if (app) render(app);
});
