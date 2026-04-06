import { store, Screen } from '../state/store.js';

export function renderTitlebar(): HTMLElement {
  const titlebar = document.createElement('div');
  titlebar.className = 'titlebar';
  titlebar.setAttribute('data-tauri-drag-region', '');

  titlebar.innerHTML = `
    <div class="titlebar-accent"></div>
    <div class="titlebar-content flex items-center justify-between w-full" data-tauri-drag-region>
      <div class="titlebar__title" data-tauri-drag-region>
        <span class="titlebar__brand">SCIP-IO</span>
      </div>
      <div class="flex items-center gap-sm" style="-webkit-app-region: no-drag; app-region: no-drag;">
        <button class="btn btn--ghost btn--sm titlebar-nav-btn" data-screen="dashboard">Dashboard</button>
        <button class="btn btn--ghost btn--sm titlebar-nav-btn" data-screen="settings">Settings</button>
      </div>
      <div class="titlebar__controls">
        <button class="titlebar__btn" id="btn-minimize">
          <svg width="10" height="1" viewBox="0 0 10 1"><rect width="10" height="1" fill="currentColor"/></svg>
        </button>
        <button class="titlebar__btn" id="btn-maximize">
          <svg width="10" height="10" viewBox="0 0 10 10"><rect x="0.5" y="0.5" width="9" height="9" stroke="currentColor" fill="none"/></svg>
        </button>
        <button class="titlebar__btn titlebar__btn--close" id="btn-close">
          <svg width="10" height="10" viewBox="0 0 10 10"><line x1="0" y1="0" x2="10" y2="10" stroke="currentColor"/><line x1="10" y1="0" x2="0" y2="10" stroke="currentColor"/></svg>
        </button>
      </div>
    </div>
  `;

  // Window control handlers — wrapped in try/catch for dev mode without Tauri
  titlebar.querySelector('#btn-minimize')?.addEventListener('click', async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().minimize();
    } catch {
      console.log('[dev] minimize not available outside Tauri');
    }
  });

  titlebar.querySelector('#btn-maximize')?.addEventListener('click', async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().toggleMaximize();
    } catch {
      console.log('[dev] maximize not available outside Tauri');
    }
  });

  titlebar.querySelector('#btn-close')?.addEventListener('click', async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().close();
    } catch {
      console.log('[dev] close not available outside Tauri');
    }
  });

  // Nav handlers
  titlebar.querySelectorAll('.titlebar-nav-btn').forEach((btn) => {
    btn.addEventListener('click', () => {
      const screen = btn.getAttribute('data-screen') as Screen;
      if (screen) {
        store.setState({ screen });
      }
    });
  });

  // Highlight active nav button
  function updateActiveNav(screen: Screen) {
    titlebar.querySelectorAll('.titlebar-nav-btn').forEach((btn) => {
      const btnScreen = btn.getAttribute('data-screen');
      if (btnScreen === screen || (screen === 'indexing' && btnScreen === 'dashboard') || (screen === 'results' && btnScreen === 'dashboard')) {
        btn.classList.add('text-cyan');
      } else {
        btn.classList.remove('text-cyan');
      }
    });
  }

  updateActiveNav(store.getState().screen);
  store.subscribe((state) => updateActiveNav(state.screen));

  return titlebar;
}
