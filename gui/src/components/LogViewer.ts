import { store, LogEntry } from '../state/store.js';

export function renderLogViewer(container: HTMLElement): () => void {
  const wrapper = document.createElement('div');
  wrapper.className = 'flex flex-col';

  // Header
  const header = document.createElement('div');
  header.className = 'log-viewer__header';
  header.innerHTML = `
    <span class="log-viewer__header-title">Output Log</span>
    <button class="btn btn--ghost btn--sm" id="log-clear-btn">Clear</button>
  `;
  wrapper.appendChild(header);

  // Log body
  const logBody = document.createElement('div');
  logBody.className = 'log-viewer';
  logBody.id = 'log-viewer-body';
  wrapper.appendChild(logBody);

  container.appendChild(wrapper);

  let autoScroll = true;

  // Track user scroll to manage auto-scroll
  logBody.addEventListener('scroll', () => {
    const atBottom = logBody.scrollHeight - logBody.scrollTop - logBody.clientHeight < 30;
    autoScroll = atBottom;
  });

  // Clear button
  const clearBtn = header.querySelector('#log-clear-btn');
  if (clearBtn) {
    clearBtn.addEventListener('click', () => {
      store.setState({ logs: [] });
    });
  }

  function renderLogs(logs: LogEntry[]) {
    logBody.innerHTML = '';
    if (logs.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'text-muted text-sm p-md';
      empty.textContent = 'No log entries yet...';
      logBody.appendChild(empty);
      return;
    }

    for (const entry of logs) {
      logBody.appendChild(createLogLine(entry));
    }

    if (autoScroll) {
      logBody.scrollTop = logBody.scrollHeight;
    }
  }

  // Initial render
  renderLogs(store.getState().logs);

  // Subscribe to changes
  const unsub = store.subscribe((state) => {
    renderLogs(state.logs);
  });

  return unsub;
}

function createLogLine(entry: LogEntry): HTMLElement {
  const line = document.createElement('div');
  const levelClass = entry.level === 'error' ? ' log-line--error' : entry.level === 'warning' ? ' log-line--warn' : '';
  line.className = `log-line${levelClass}`;

  const time = document.createElement('span');
  time.className = 'log-line__time';
  time.textContent = entry.timestamp;

  const level = document.createElement('span');
  const levelColorClass = getLevelClass(entry.level);
  level.className = `log-line__level ${levelColorClass}`;
  level.textContent = entry.level.toUpperCase();

  const msg = document.createElement('span');
  msg.className = 'log-line__msg';
  msg.textContent = entry.message;

  line.appendChild(time);
  line.appendChild(level);
  line.appendChild(msg);

  return line;
}

function getLevelClass(level: LogEntry['level']): string {
  switch (level) {
    case 'info':
      return 'log-line__level--info';
    case 'success':
      return 'log-line__level--info';
    case 'error':
      return 'log-line__level--error';
    case 'warning':
      return 'log-line__level--warn';
    default:
      return 'log-line__level--info';
  }
}
