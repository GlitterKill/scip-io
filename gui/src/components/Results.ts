import { store, addLog } from '../state/store.js';

export function renderResults(container: HTMLElement): void {
  const wrapper = document.createElement('div');
  wrapper.className = 'content-wrapper section';
  container.appendChild(wrapper);

  const state = store.getState();
  const results = state.results;

  // Header
  const header = document.createElement('div');
  header.className = 'flex items-center justify-between mb-lg animate-fade-in';
  header.innerHTML = `
    <div>
      <h2 class="text-xl text-success">Indexing Complete</h2>
      <p class="text-sm text-secondary mt-xs">Your SCIP index has been generated successfully.</p>
    </div>
  `;
  wrapper.appendChild(header);

  if (!results) {
    const empty = document.createElement('div');
    empty.className = 'panel';
    empty.innerHTML = `
      <div class="empty-state">
        <div class="empty-state__icon text-muted">!</div>
        <div class="empty-state__title">No results available</div>
        <div class="empty-state__description">Run an indexing operation to see results here.</div>
      </div>
    `;
    wrapper.appendChild(empty);
    wrapper.appendChild(renderBackButton());
    return;
  }

  // Summary stats
  wrapper.appendChild(renderSummaryStats(results));

  // Per-language results table
  wrapper.appendChild(renderLanguageResults(results));

  // Actions
  wrapper.appendChild(renderActions(results));
}

// ----- Summary Stats -----

function renderSummaryStats(results: NonNullable<ReturnType<typeof store.getState>['results']>): HTMLElement {
  const panel = document.createElement('div');
  panel.className = 'panel animate-fade-in';

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">Summary</span>
    <div class="section-header__line"></div>
  `;
  panel.appendChild(sectionHeader);

  const grid = document.createElement('div');
  grid.className = 'grid grid-4 gap-lg';

  const stats = [
    { label: 'Output File', value: results.output, color: 'text-cyan' },
    { label: 'Total Files', value: results.totalFiles.toLocaleString(), color: 'text-primary' },
    { label: 'Total Symbols', value: results.totalSymbols.toLocaleString(), color: 'text-primary' },
    { label: 'Duration', value: formatDuration(results.totalDuration), color: 'text-primary' },
  ];

  stats.forEach((stat, idx) => {
    const card = document.createElement('div');
    card.className = `panel panel--inset panel--compact animate-fade-in stagger-${idx + 1}`;
    card.innerHTML = `
      <div class="text-xs text-muted uppercase mb-xs mono">${stat.label}</div>
      <div class="text-lg font-semibold ${stat.color} tabular-nums">${stat.value}</div>
    `;
    grid.appendChild(card);
  });

  panel.appendChild(grid);

  // Output size
  if (results.outputSize > 0) {
    const sizeInfo = document.createElement('div');
    sizeInfo.className = 'flex items-center gap-sm mt-lg text-sm text-secondary';
    sizeInfo.innerHTML = `
      <span>Output size:</span>
      <span class="text-cyan mono">${formatBytes(results.outputSize)}</span>
      <span class="separator--vertical"></span>
      <span>Languages:</span>
      <span class="text-cyan mono">${results.languages.length}</span>
    `;
    panel.appendChild(sizeInfo);
  }

  return panel;
}

// ----- Per-Language Results Table -----

function renderLanguageResults(results: NonNullable<ReturnType<typeof store.getState>['results']>): HTMLElement {
  const panel = document.createElement('div');
  panel.className = 'panel panel--flush animate-fade-in stagger-3';

  const headerRow = document.createElement('div');
  headerRow.className = 'panel__header px-xl pt-lg';
  headerRow.innerHTML = `
    <div>
      <div class="panel__title">Per-Language Results</div>
      <div class="panel__subtitle">Breakdown of indexing results by language</div>
    </div>
  `;
  panel.appendChild(headerRow);

  if (results.languages.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'p-xl text-center text-muted text-sm';
    empty.textContent = 'No per-language data available.';
    panel.appendChild(empty);
    return panel;
  }

  const tableContainer = document.createElement('div');
  tableContainer.className = 'table-container';

  const table = document.createElement('table');
  table.className = 'table table--striped table--hover';

  table.innerHTML = `
    <thead>
      <tr>
        <th>Language</th>
        <th class="col-num">Files</th>
        <th class="col-num">Symbols</th>
        <th class="col-num">Duration</th>
        <th class="col-num">Symbols/File</th>
      </tr>
    </thead>
  `;

  const tbody = document.createElement('tbody');
  results.languages.forEach((lang) => {
    const tr = document.createElement('tr');
    const ratio = lang.files > 0 ? (lang.symbols / lang.files).toFixed(1) : '0';

    tr.innerHTML = `
      <td class="text-primary font-medium">${escapeHtml(lang.name)}</td>
      <td class="col-num tabular-nums">${lang.files.toLocaleString()}</td>
      <td class="col-num tabular-nums">${lang.symbols.toLocaleString()}</td>
      <td class="col-num tabular-nums">${formatDuration(lang.duration)}</td>
      <td class="col-num tabular-nums text-cyan">${ratio}</td>
    `;
    tbody.appendChild(tr);
  });

  // Totals row
  const totalTr = document.createElement('tr');
  totalTr.innerHTML = `
    <td class="font-semibold text-cyan">Total</td>
    <td class="col-num tabular-nums font-semibold">${results.totalFiles.toLocaleString()}</td>
    <td class="col-num tabular-nums font-semibold">${results.totalSymbols.toLocaleString()}</td>
    <td class="col-num tabular-nums font-semibold">${formatDuration(results.totalDuration)}</td>
    <td class="col-num tabular-nums font-semibold text-cyan">${results.totalFiles > 0 ? (results.totalSymbols / results.totalFiles).toFixed(1) : '0'}</td>
  `;
  totalTr.style.borderTop = '2px solid var(--accent-cyan)';
  tbody.appendChild(totalTr);

  table.appendChild(tbody);
  tableContainer.appendChild(table);
  panel.appendChild(tableContainer);

  return panel;
}

// ----- Action Buttons -----

function renderActions(results: NonNullable<ReturnType<typeof store.getState>['results']>): HTMLElement {
  const actions = document.createElement('div');
  actions.className = 'flex gap-sm mt-lg animate-fade-in stagger-5';

  // Back to Dashboard
  const backBtn = document.createElement('button');
  backBtn.className = 'btn btn--secondary';
  backBtn.textContent = 'Back to Dashboard';
  backBtn.addEventListener('click', () => {
    store.setState({ screen: 'dashboard' });
  });
  actions.appendChild(backBtn);

  // Index Again
  const againBtn = document.createElement('button');
  againBtn.className = 'btn btn--primary';
  againBtn.textContent = 'Index Again';
  againBtn.addEventListener('click', () => {
    store.setState({ screen: 'dashboard', results: null, logs: [], overallProgress: 0, pipelineStep: 'detect' });
  });
  actions.appendChild(againBtn);

  // Open Output
  const openBtn = document.createElement('button');
  openBtn.className = 'btn btn--primary-filled';
  openBtn.textContent = 'Open Output Location';
  openBtn.addEventListener('click', async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('reveal_in_explorer', { path: results.output });
    } catch (e) {
      addLog('warning', 'Failed to open output location: ' + String(e));
    }
  });
  actions.appendChild(openBtn);

  return actions;
}

function renderBackButton(): HTMLElement {
  const actions = document.createElement('div');
  actions.className = 'flex gap-sm mt-lg';
  const backBtn = document.createElement('button');
  backBtn.className = 'btn btn--secondary';
  backBtn.textContent = 'Back to Dashboard';
  backBtn.addEventListener('click', () => {
    store.setState({ screen: 'dashboard' });
  });
  actions.appendChild(backBtn);
  return actions;
}

// ----- Utilities -----

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const secs = (seconds % 60).toFixed(0);
  return `${minutes}m ${secs}s`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function escapeHtml(str: string): string {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}
