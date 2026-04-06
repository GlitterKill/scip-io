import { store, addLog } from '../state/store.js';
import type { IndexerProgress, IndexingResult, LogEntry, AppState } from '../state/store.js';
import { cancelIndexing, onProgress } from '../bridge/tauri.js';
import { renderProgressBar } from './ProgressBar.js';
import { renderLogViewer } from './LogViewer.js';

const PIPELINE_STEPS = ['detect', 'download', 'index', 'merge', 'done'] as const;
const PIPELINE_LABELS: Record<string, string> = {
  detect: 'Detect Languages',
  download: 'Download Indexers',
  index: 'Run Indexers',
  merge: 'Merge Indices',
  done: 'Complete',
};

let progressUnlisten: (() => void) | null = null;

export function renderIndexProgress(container: HTMLElement): void {
  const wrapper = document.createElement('div');
  wrapper.className = 'content-wrapper section';
  container.appendChild(wrapper);

  // Header
  const header = document.createElement('div');
  header.className = 'flex items-center justify-between mb-lg';
  header.innerHTML = `
    <div>
      <h2 class="text-xl">Indexing in Progress</h2>
      <p class="text-sm text-secondary mt-xs">Processing your project...</p>
    </div>
  `;
  wrapper.appendChild(header);

  // Overall progress panel
  const overallPanel = document.createElement('div');
  overallPanel.className = 'panel animate-fade-in';
  overallPanel.id = 'overall-progress-panel';
  wrapper.appendChild(overallPanel);
  renderOverallProgress(overallPanel);

  // Pipeline steps panel
  const pipelinePanel = document.createElement('div');
  pipelinePanel.className = 'panel animate-fade-in stagger-2';
  pipelinePanel.id = 'pipeline-panel';
  wrapper.appendChild(pipelinePanel);
  renderPipelineSteps(pipelinePanel);

  // Per-language progress cards
  const langProgressContainer = document.createElement('div');
  langProgressContainer.className = 'grid grid-auto-md gap-md';
  langProgressContainer.id = 'lang-progress-container';
  wrapper.appendChild(langProgressContainer);
  renderLanguageCards(langProgressContainer);

  // Log viewer
  const logSection = document.createElement('div');
  logSection.className = 'animate-fade-in stagger-4';
  logSection.id = 'indexing-log-section';
  wrapper.appendChild(logSection);
  const logUnsub = renderLogViewer(logSection);

  // Cancel button
  const cancelRow = document.createElement('div');
  cancelRow.className = 'flex justify-end mt-lg';
  const cancelBtn = document.createElement('button');
  cancelBtn.className = 'btn btn--danger';
  cancelBtn.id = 'cancel-indexing-btn';
  cancelBtn.textContent = 'Cancel';
  cancelBtn.addEventListener('click', handleCancel);
  cancelRow.appendChild(cancelBtn);
  wrapper.appendChild(cancelRow);

  // Subscribe to state changes
  const unsub = store.subscribe((state) => {
    const op = document.getElementById('overall-progress-panel');
    if (op) {
      op.innerHTML = '';
      renderOverallProgress(op);
    }

    const pp = document.getElementById('pipeline-panel');
    if (pp) {
      pp.innerHTML = '';
      renderPipelineSteps(pp);
    }

    const lp = document.getElementById('lang-progress-container');
    if (lp) {
      lp.innerHTML = '';
      renderLanguageCards(lp);
    }

    // If indexing finished, transition to results
    if (!state.isIndexing && state.pipelineStep === 'done' && state.results) {
      store.setState({ screen: 'results' });
    }

    // Update cancel button
    const cb = document.getElementById('cancel-indexing-btn') as HTMLButtonElement | null;
    if (cb) {
      cb.disabled = !state.isIndexing;
    }
  });

  // Listen for progress events from Tauri backend
  setupProgressListener();

  // Cleanup on screen change — we store unsub references
  const screenUnsub = store.subscribe((state) => {
    if (state.screen !== 'indexing') {
      unsub();
      logUnsub();
      screenUnsub();
      teardownProgressListener();
    }
  });
}

// ----- Overall Progress -----

function renderOverallProgress(container: HTMLElement): void {
  const state = store.getState();

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">Overall Progress</span>
    <div class="section-header__line"></div>
  `;
  container.appendChild(sectionHeader);

  const bar = renderProgressBar({
    label: `Step: ${PIPELINE_LABELS[state.pipelineStep] || state.pipelineStep}`,
    value: state.overallProgress,
    size: 'lg',
    indeterminate: state.isIndexing && state.overallProgress === 0,
  });
  container.appendChild(bar);
}

// ----- Pipeline Steps -----

function renderPipelineSteps(container: HTMLElement): void {
  const state = store.getState();
  const currentIdx = PIPELINE_STEPS.indexOf(state.pipelineStep);

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">Pipeline</span>
    <div class="section-header__line"></div>
  `;
  container.appendChild(sectionHeader);

  const stepsRow = document.createElement('div');
  stepsRow.className = 'flex items-center gap-md';

  PIPELINE_STEPS.forEach((step, idx) => {
    const stepEl = document.createElement('div');
    stepEl.className = 'flex items-center gap-xs';

    const indicator = document.createElement('div');
    const allDone = state.pipelineStep === 'done';
    if (idx < currentIdx || allDone) {
      // Completed
      indicator.className = 'flex items-center justify-center rounded-full';
      indicator.style.width = '24px';
      indicator.style.height = '24px';
      indicator.style.background = 'var(--color-success-dim)';
      indicator.style.border = '1px solid var(--color-success)';
      indicator.innerHTML = `<svg width="12" height="12" viewBox="0 0 12 12" fill="none"><path d="M2 6l3 3 5-5" stroke="var(--color-success)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
    } else if (idx === currentIdx) {
      // Current
      indicator.className = 'flex items-center justify-center rounded-full animate-glow';
      indicator.style.width = '24px';
      indicator.style.height = '24px';
      indicator.style.background = 'var(--accent-cyan-dim)';
      indicator.style.border = '1px solid var(--accent-cyan)';
      indicator.innerHTML = `<span class="spinner spinner--sm"></span>`;
    } else {
      // Pending
      indicator.className = 'flex items-center justify-center rounded-full';
      indicator.style.width = '24px';
      indicator.style.height = '24px';
      indicator.style.background = 'var(--bg-surface)';
      indicator.style.border = '1px solid var(--border-default)';
    }

    stepEl.appendChild(indicator);

    const label = document.createElement('span');
    label.className = idx === currentIdx ? 'text-sm text-cyan font-medium' : 'text-sm text-muted';
    label.textContent = PIPELINE_LABELS[step];
    stepEl.appendChild(label);

    stepsRow.appendChild(stepEl);

    // Connector line between steps
    if (idx < PIPELINE_STEPS.length - 1) {
      const line = document.createElement('div');
      line.className = 'flex-1';
      line.style.height = '1px';
      line.style.background = idx < currentIdx ? 'var(--color-success)' : 'var(--border-default)';
      line.style.minWidth = '20px';
      stepsRow.appendChild(line);
    }
  });

  container.appendChild(stepsRow);
}

// ----- Per-Language Progress Cards -----

function renderLanguageCards(container: HTMLElement): void {
  const state = store.getState();
  const entries: IndexerProgress[] = Array.from(state.indexerProgress.values());

  if (entries.length === 0) {
    // Show cards for selected languages even before progress starts
    const selectedLangs = state.languages.filter((l) => l.selected);
    for (const lang of selectedLangs) {
      container.appendChild(createLangCard({
        language: lang.name,
        status: 'queued',
        progress: 0,
        message: 'Waiting...',
      }));
    }
    return;
  }

  for (const entry of entries) {
    container.appendChild(createLangCard(entry));
  }
}

function createLangCard(entry: IndexerProgress): HTMLElement {
  const card = document.createElement('div');
  const isActive = entry.status === 'running' || entry.status === 'downloading';
  const isDone = entry.status === 'done';
  const isFailed = entry.status === 'failed';

  card.className = `panel panel--compact animate-fade-in${isActive ? ' panel--active' : ''}`;

  // Language name + status
  const headerRow = document.createElement('div');
  headerRow.className = 'flex items-center justify-between mb-sm';

  const langName = document.createElement('span');
  langName.className = 'font-medium text-primary';
  langName.textContent = entry.language;
  headerRow.appendChild(langName);

  const statusBadge = document.createElement('span');
  statusBadge.className = `badge badge--${getStatusBadgeClass(entry.status)}`;
  statusBadge.innerHTML = `<span class="badge__dot"></span> ${formatStatus(entry.status)}`;
  headerRow.appendChild(statusBadge);

  card.appendChild(headerRow);

  // Progress bar
  const bar = renderProgressBar({
    value: entry.progress,
    size: 'sm',
    showValue: entry.status !== 'queued',
    indeterminate: entry.status === 'downloading',
  });
  card.appendChild(bar);

  // Status message
  const msg = document.createElement('div');
  msg.className = `text-xs mt-sm ${isFailed ? 'text-error' : isDone ? 'text-success' : 'text-muted'}`;
  msg.textContent = entry.message;
  card.appendChild(msg);

  // Stats (if available)
  if (entry.files || entry.symbols || entry.duration) {
    const stats = document.createElement('div');
    stats.className = 'flex gap-md mt-sm text-xs text-secondary';
    if (entry.files) stats.innerHTML += `<span>Files: <span class="text-primary tabular-nums">${entry.files}</span></span>`;
    if (entry.symbols) stats.innerHTML += `<span>Symbols: <span class="text-primary tabular-nums">${entry.symbols}</span></span>`;
    if (entry.duration) stats.innerHTML += `<span>Time: <span class="text-primary tabular-nums">${(entry.duration / 1000).toFixed(1)}s</span></span>`;
    card.appendChild(stats);
  }

  return card;
}

function getStatusBadgeClass(status: IndexerProgress['status']): string {
  switch (status) {
    case 'queued': return 'not-installed';
    case 'downloading': return 'running';
    case 'running': return 'running';
    case 'done': return 'installed';
    case 'failed': return 'error';
    default: return 'not-installed';
  }
}

function formatStatus(status: IndexerProgress['status']): string {
  switch (status) {
    case 'queued': return 'Queued';
    case 'downloading': return 'Downloading';
    case 'running': return 'Running';
    case 'done': return 'Done';
    case 'failed': return 'Failed';
    default: return status;
  }
}

// ----- Progress Event Listener -----

async function setupProgressListener() {
  try {
    progressUnlisten = await onProgress((event) => {
      handleProgressEvent(event);
    });
  } catch (err) {
    console.log('[dev] Progress listener not available outside Tauri');
    // Simulate progress in dev mode
    simulateProgress();
  }
}

function teardownProgressListener() {
  if (progressUnlisten) {
    progressUnlisten();
    progressUnlisten = null;
  }
}

function handleProgressEvent(event: Record<string, unknown>) {
  // Parse the progress event from the Rust backend
  const kind = event.kind as string | undefined;
  const language = event.language as string | undefined;

  if (kind === 'pipeline_step') {
    const step = event.step as string;
    const progress = event.progress as number;
    store.setState({
      pipelineStep: step as AppState['pipelineStep'],
      overallProgress: progress,
    });
    addLog('info', `Pipeline step: ${PIPELINE_LABELS[step] || step}`);
  }

  if (kind === 'language_progress' && language) {
    const map = new Map(store.getState().indexerProgress);
    map.set(language, {
      language,
      status: event.status as IndexerProgress['status'],
      progress: (event.progress as number) || 0,
      message: (event.message as string) || '',
      duration: event.duration as number | undefined,
      symbols: event.symbols as number | undefined,
      files: event.files as number | undefined,
    });
    store.setState({ indexerProgress: map });
  }

  if (kind === 'indexing_complete') {
    store.setState({
      isIndexing: false,
      pipelineStep: 'done',
      overallProgress: 100,
      results: {
        output: (event.output as string) || 'index.scip',
        totalFiles: (event.total_files as number) || 0,
        totalSymbols: (event.total_symbols as number) || 0,
        totalDuration: (event.total_duration as number) || 0,
        languages: (event.languages as IndexingResult['languages']) || [],
        outputSize: (event.output_size as number) || 0,
      },
    });
    addLog('success', 'Indexing complete!');
  }

  if (kind === 'log') {
    const level = (event.level as string) || 'info';
    const message = (event.message as string) || '';
    addLog(level as LogEntry['level'], message);
  }

  if (kind === 'error') {
    addLog('error', (event.message as string) || 'Unknown error');
  }
}

// ----- Dev Mode Simulation -----

function simulateProgress() {
  const state = store.getState();
  if (!state.isIndexing) return;

  const selectedLangs = state.languages.filter((l) => l.selected).map((l) => l.name);
  if (selectedLangs.length === 0) return;

  let step = 0;
  const steps: Array<[string, number]> = [
    ['detect', 15],
    ['download', 40],
    ['index', 85],
    ['merge', 95],
    ['done', 100],
  ];

  const interval = setInterval(() => {
    if (!store.getState().isIndexing || step >= steps.length) {
      clearInterval(interval);
      return;
    }

    const [stepName, progress] = steps[step];
    store.setState({
      pipelineStep: stepName as AppState['pipelineStep'],
      overallProgress: progress,
    });
    addLog('info', `[sim] Pipeline: ${PIPELINE_LABELS[stepName]}`);

    // Update per-language progress
    const map = new Map(store.getState().indexerProgress);
    selectedLangs.forEach((lang, idx) => {
      const langProgress = Math.min(100, progress + idx * 5);
      const status: IndexerProgress['status'] = langProgress >= 100 ? 'done' : progress > 40 ? 'running' : progress > 15 ? 'downloading' : 'queued';
      map.set(lang, {
        language: lang,
        status,
        progress: langProgress,
        message: status === 'done' ? 'Completed' : status === 'running' ? 'Indexing files...' : status === 'downloading' ? 'Downloading binary...' : 'Waiting...',
        files: status === 'running' || status === 'done' ? Math.floor(langProgress * 1.2) : undefined,
        symbols: status === 'done' ? Math.floor(langProgress * 15) : undefined,
        duration: status === 'done' ? 3200 + idx * 800 : undefined,
      });
    });
    store.setState({ indexerProgress: map });

    step++;

    if (stepName === 'done') {
      clearInterval(interval);
      store.setState({
        isIndexing: false,
        results: {
          output: store.getState().settings.outputFile || 'index.scip',
          totalFiles: selectedLangs.length * 120,
          totalSymbols: selectedLangs.length * 1500,
          totalDuration: 4500 + selectedLangs.length * 800,
          languages: selectedLangs.map((lang, idx) => ({
            name: lang,
            files: 100 + idx * 20,
            symbols: 1200 + idx * 300,
            duration: 3200 + idx * 800,
          })),
          outputSize: 524288,
        },
      });
      addLog('success', '[sim] Indexing complete!');
    }
  }, 1500);
}

// ----- Cancel Handler -----

async function handleCancel() {
  addLog('warning', 'Cancelling indexing...');
  try {
    await cancelIndexing();
  } catch {
    console.log('[dev] Cancel not available outside Tauri');
  }
  store.setState({ isIndexing: false });
  addLog('warning', 'Indexing cancelled by user');
  store.setState({ screen: 'dashboard' });
}
