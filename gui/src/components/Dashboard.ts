import { store, addLog } from '../state/store.js';
import {
  detectLanguages,
  getIndexerStatus,
  installIndexer,
  startIndexing,
  uninstallIndexer,
} from '../bridge/tauri.js';
import { renderStatusBadge } from './StatusBadge.js';

const pendingIndexerActions = new Set<string>();

type IndexerRow = ReturnType<typeof store.getState>['indexers'][number];

export function initApp(_container: HTMLElement): void {
  // Legacy stub; the real entry point is now main.ts
}

export function renderDashboard(container: HTMLElement): void {
  const wrapper = document.createElement('div');
  wrapper.className = 'content-wrapper section';
  container.appendChild(wrapper);

  // Page header
  const header = document.createElement('div');
  header.className = 'flex items-center justify-between mb-xl';
  header.innerHTML = `
    <div>
      <h1 class="text-2xl animate-neon">SCIP-IO</h1>
      <p class="text-sm text-secondary mt-xs">SCIP Index Orchestrator</p>
    </div>
  `;
  wrapper.appendChild(header);

  // Project path section
  wrapper.appendChild(renderProjectPathSection());

  // Detected languages section
  const langSection = document.createElement('div');
  langSection.id = 'languages-section';
  wrapper.appendChild(langSection);
  renderLanguagesSection(langSection);

  // Indexer status table
  const indexerSection = document.createElement('div');
  indexerSection.id = 'indexer-section';
  wrapper.appendChild(indexerSection);
  renderIndexerTable(indexerSection);

  // Subscribe to state changes for reactive sections
  store.subscribe((state) => {
    const langEl = wrapper.querySelector('#languages-section');
    if (langEl) {
      langEl.innerHTML = '';
      renderLanguagesSection(langEl as HTMLElement);
    }
    const idxEl = wrapper.querySelector('#indexer-section');
    if (idxEl) {
      idxEl.innerHTML = '';
      renderIndexerTable(idxEl as HTMLElement);
    }
    // Disable index button during indexing
    const indexBtn = wrapper.querySelector('#btn-index-all') as HTMLButtonElement | null;
    if (indexBtn) {
      indexBtn.disabled = state.isIndexing;
    }
  });

  // Fetch initial data
  handleDetect();
  handleFetchIndexers();
}

// ----- Project Path Section -----

function renderProjectPathSection(): HTMLElement {
  const panel = document.createElement('div');
  panel.className = 'panel animate-fade-in';

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">Project Path</span>
    <div class="section-header__line"></div>
  `;
  panel.appendChild(sectionHeader);

  const row = document.createElement('div');
  row.className = 'flex gap-sm items-end';

  const field = document.createElement('div');
  field.className = 'form-field flex-1';
  field.innerHTML = `
    <label class="form-label">Working Directory</label>
    <input class="input" type="text" id="project-path-input"
           value="${escapeHtml(store.getState().projectPath)}"
           placeholder="Path to project root..." />
  `;
  row.appendChild(field);

  const browseBtn = document.createElement('button');
  browseBtn.className = 'btn btn--secondary';
  browseBtn.textContent = 'Browse';
  browseBtn.addEventListener('click', handleBrowse);
  row.appendChild(browseBtn);

  panel.appendChild(row);

  // Action buttons
  const actions = document.createElement('div');
  actions.className = 'flex gap-sm mt-lg';

  const detectBtn = document.createElement('button');
  detectBtn.className = 'btn btn--secondary';
  detectBtn.textContent = 'Detect Languages';
  detectBtn.addEventListener('click', handleDetect);
  actions.appendChild(detectBtn);

  const indexBtn = document.createElement('button');
  indexBtn.className = 'btn btn--primary-filled btn--lg';
  indexBtn.id = 'btn-index-all';
  indexBtn.textContent = 'Index All';
  indexBtn.disabled = store.getState().isIndexing;
  indexBtn.addEventListener('click', handleIndexAll);
  actions.appendChild(indexBtn);

  panel.appendChild(actions);

  return panel;
}

// ----- Detected Languages Section -----

function renderLanguagesSection(container: HTMLElement): void {
  const state = store.getState();

  const panel = document.createElement('div');
  panel.className = 'panel animate-fade-in stagger-2';

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">Detected Languages</span>
    ${state.languages.length > 0 ? `<span class="section-header__count">${state.languages.length}</span>` : ''}
    <div class="section-header__line"></div>
  `;
  panel.appendChild(sectionHeader);

  if (state.languages.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'empty-state';
    empty.innerHTML = `
      <div class="empty-state__icon text-muted">?</div>
      <div class="empty-state__title">No languages detected</div>
      <div class="empty-state__description">Click "Detect Languages" to scan the project for supported languages.</div>
    `;
    panel.appendChild(empty);
  } else {
    const chipGroup = document.createElement('div');
    chipGroup.className = 'chip-group';

    state.languages.forEach((lang, index) => {
      const chip = document.createElement('div');
      chip.className = `chip${lang.selected ? ' chip--selected' : ''} animate-fade-in stagger-${Math.min(index + 1, 8)}`;
      chip.innerHTML = `
        <span class="chip__checkbox"></span>
        <span class="chip__label">${escapeHtml(lang.name)}</span>
      `;
      chip.title = lang.evidence;
      chip.addEventListener('click', () => {
        const langs = store.getState().languages.map((l) =>
          l.name === lang.name ? { ...l, selected: !l.selected } : l
        );
        store.setState({ languages: langs });
      });
      chipGroup.appendChild(chip);
    });

    panel.appendChild(chipGroup);

    // Select/deselect all
    const selectActions = document.createElement('div');
    selectActions.className = 'flex gap-sm mt-md';

    const selectAllBtn = document.createElement('button');
    selectAllBtn.className = 'btn btn--ghost btn--sm';
    selectAllBtn.textContent = 'Select All';
    selectAllBtn.addEventListener('click', () => {
      const langs = store.getState().languages.map((l) => ({ ...l, selected: true }));
      store.setState({ languages: langs });
    });
    selectActions.appendChild(selectAllBtn);

    const deselectAllBtn = document.createElement('button');
    deselectAllBtn.className = 'btn btn--ghost btn--sm';
    deselectAllBtn.textContent = 'Deselect All';
    deselectAllBtn.addEventListener('click', () => {
      const langs = store.getState().languages.map((l) => ({ ...l, selected: false }));
      store.setState({ languages: langs });
    });
    selectActions.appendChild(deselectAllBtn);

    panel.appendChild(selectActions);
  }

  container.appendChild(panel);
}

// ----- Indexer Status Table -----

function renderIndexerTable(container: HTMLElement): void {
  const state = store.getState();

  const panel = document.createElement('div');
  panel.className = 'panel panel--flush animate-fade-in stagger-3';

  const headerRow = document.createElement('div');
  headerRow.className = 'panel__header px-xl pt-lg';
  headerRow.innerHTML = `
    <div>
      <div class="panel__title">Indexer Status</div>
      <div class="panel__subtitle">Available SCIP indexers and their installation state</div>
    </div>
  `;
  panel.appendChild(headerRow);

  if (state.indexers.length === 0) {
    const loading = document.createElement('div');
    loading.className = 'flex items-center justify-center gap-sm p-xl text-secondary';
    loading.innerHTML = `<span class="spinner"></span> Loading indexer status...`;
    panel.appendChild(loading);
  } else {
    const tableContainer = document.createElement('div');
    tableContainer.className = 'table-container';

    const table = document.createElement('table');
    table.className = 'table table--striped table--hover';

    table.innerHTML = `
      <thead>
        <tr>
          <th>Indexer</th>
          <th>Language</th>
          <th>Version</th>
          <th class="col-status">Status</th>
          <th class="col-actions">Action</th>
        </tr>
      </thead>
    `;

    const tbody = document.createElement('tbody');
    state.indexers.forEach((idx) => {
      const tr = document.createElement('tr');

      const tdName = document.createElement('td');
      tdName.className = 'text-primary font-medium';
      const nameLine = document.createElement('div');
      nameLine.textContent = idx.name;
      tdName.appendChild(nameLine);
      if (idx.coveredBy) {
        const coverageLine = document.createElement('div');
        coverageLine.className = 'text-xs text-muted mt-xs';
        coverageLine.textContent = `Covered by ${idx.coveredBy}`;
        tdName.appendChild(coverageLine);
      }
      tr.appendChild(tdName);

      const tdLang = document.createElement('td');
      tdLang.textContent = idx.language;
      tr.appendChild(tdLang);

      const tdVersion = document.createElement('td');
      tdVersion.className = 'text-cyan mono text-sm';
      tdVersion.textContent = idx.version;
      tr.appendChild(tdVersion);

      const tdStatus = document.createElement('td');
      tdStatus.className = 'col-status';
      const badge = renderStatusBadge(idx.installed ? 'installed' : 'not-installed');
      tdStatus.appendChild(badge);
      tr.appendChild(tdStatus);

      const tdAction = document.createElement('td');
      tdAction.className = 'col-actions';
      tdAction.appendChild(renderIndexerActionButton(idx));
      tr.appendChild(tdAction);

      tbody.appendChild(tr);
    });

    table.appendChild(tbody);
    tableContainer.appendChild(table);
    panel.appendChild(tableContainer);
  }

  container.appendChild(panel);
}

// ----- Handlers -----

async function handleBrowse() {
  // Check if running inside Tauri
  const isTauri = '__TAURI_INTERNALS__' in window;

  if (isTauri) {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({ directory: true, multiple: false, title: 'Select Project Directory' });
      if (selected && typeof selected === 'string') {
        applyProjectPath(selected);
      }
      return;
    } catch {
      // fall through to browser fallback
    }
  }

  // Browser fallback: show inline modal
  showPathModal();
}

function showPathModal() {
  // Remove any existing modal
  document.getElementById('path-modal')?.remove();

  const overlay = document.createElement('div');
  overlay.id = 'path-modal';
  overlay.className = 'dialog-overlay';

  const dialog = document.createElement('div');
  dialog.className = 'dialog animate-fade-in';
  dialog.innerHTML = `
    <div class="dialog__header">Enter Project Path</div>
    <div class="dialog__body">
      <div class="form-field">
        <label class="form-label">Project Directory</label>
        <input class="input" type="text" id="modal-path-input"
               value="${escapeHtml(store.getState().projectPath)}"
               placeholder="/path/to/project" autofocus />
      </div>
    </div>
    <div class="dialog__footer flex gap-sm justify-end">
      <button class="btn btn--ghost" id="modal-cancel">Cancel</button>
      <button class="btn btn--primary" id="modal-confirm">Confirm</button>
    </div>
  `;

  overlay.appendChild(dialog);
  document.body.appendChild(overlay);

  const input = document.getElementById('modal-path-input') as HTMLInputElement;
  input?.focus();
  input?.select();

  const close = () => overlay.remove();

  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) close();
  });
  document.getElementById('modal-cancel')?.addEventListener('click', close);
  document.getElementById('modal-confirm')?.addEventListener('click', () => {
    const val = input?.value?.trim();
    if (val) applyProjectPath(val);
    close();
  });
  input?.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      const val = input.value.trim();
      if (val) applyProjectPath(val);
      close();
    } else if (e.key === 'Escape') {
      close();
    }
  });
}

function applyProjectPath(path: string) {
  store.setState({ projectPath: path });
  const input = document.getElementById('project-path-input') as HTMLInputElement | null;
  if (input) input.value = path;
  addLog('info', `Project path set to: ${path}`);
  handleDetect();
}

async function handleDetect() {
  const input = document.getElementById('project-path-input') as HTMLInputElement | null;
  if (input) {
    store.setState({ projectPath: input.value });
  }

  const path = store.getState().projectPath;
  addLog('info', `Detecting languages in: ${path}`);

  try {
    const result = await detectLanguages(path);
    const languages = result.map((lang) => ({
      name: lang.name,
      evidence: lang.evidence,
      selected: true,
    }));
    store.setState({ languages });
    addLog('success', `Detected ${languages.length} language(s): ${languages.map((l) => l.name).join(', ')}`);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    addLog('error', `Detection failed: ${message}`);
    // Provide mock data in dev mode so the UI is testable
    if (!store.getState().languages.length) {
      store.setState({
        languages: [
          { name: 'TypeScript', evidence: 'tsconfig.json, package.json', selected: true },
          { name: 'Rust', evidence: 'Cargo.toml', selected: true },
          { name: 'Go', evidence: 'go.mod', selected: false },
        ],
      });
    }
  }
}

async function handleFetchIndexers() {
  try {
    const result = await getIndexerStatus();
    const indexers = result.map((idx) => ({
      name: idx.name,
      language: idx.language,
      version: idx.version,
      installed: idx.installed,
      installable: idx.installable,
      managed: idx.managed,
      installedPath: idx.installed_path,
      actionIndexer: idx.action_indexer,
      coveredBy: idx.covered_by,
    }));
    store.setState({ indexers });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    addLog('warning', `Could not fetch indexer status: ${message}`);
    // Mock data for dev
    if (!store.getState().indexers.length) {
      store.setState({
        indexers: [
          { name: 'scip-typescript', language: 'typescript, javascript', version: '0.4.0', installed: true, installable: true, managed: true, installedPath: null, actionIndexer: 'scip-typescript', coveredBy: null },
          { name: 'rust-analyzer', language: 'rust', version: '2026-03-30', installed: false, installable: true, managed: false, installedPath: null, actionIndexer: 'rust-analyzer', coveredBy: null },
          { name: 'scip-go', language: 'go', version: 'v0.1.26', installed: false, installable: true, managed: false, installedPath: null, actionIndexer: 'scip-go', coveredBy: null },
          { name: 'scip-java', language: 'java, scala', version: 'v0.12.3', installed: false, installable: true, managed: false, installedPath: null, actionIndexer: 'scip-java', coveredBy: null },
          { name: 'scip-python', language: 'python', version: '0.6.6', installed: true, installable: true, managed: true, installedPath: null, actionIndexer: 'scip-python', coveredBy: null },
          { name: 'scip-dotnet', language: 'csharp', version: '0.2.13', installed: false, installable: true, managed: false, installedPath: null, actionIndexer: 'scip-dotnet', coveredBy: null },
          { name: 'scip-ruby', language: 'ruby', version: 'v0.4.7', installed: false, installable: true, managed: false, installedPath: null, actionIndexer: 'scip-ruby', coveredBy: null },
          { name: 'scip-kotlin', language: 'kotlin', version: '0.6.0', installed: false, installable: true, managed: false, installedPath: null, actionIndexer: 'scip-java', coveredBy: 'scip-java' },
          { name: 'scip-clang', language: 'cpp', version: 'v0.4.0', installed: false, installable: true, managed: false, installedPath: null, actionIndexer: 'scip-clang', coveredBy: null },
        ],
      });
    }
  }
}

function renderIndexerActionButton(idx: IndexerRow): HTMLButtonElement {
  const isPending = pendingIndexerActions.has(idx.actionIndexer);
  const button = document.createElement('button');
  button.className = idx.installed ? 'btn btn--danger btn--sm' : 'btn btn--primary btn--sm';
  button.textContent = isPending ? 'Working...' : idx.installed ? 'Uninstall' : 'Install';
  button.disabled = isPending || !canRunIndexerAction(idx);
  button.title = getIndexerActionTitle(idx);
  button.setAttribute('aria-label', getIndexerActionLabel(idx, button.textContent));

  button.addEventListener('click', () => {
    if (idx.installed) {
      handleUninstallIndexer(idx);
    } else {
      handleInstallIndexer(idx);
    }
  });

  return button;
}

function canRunIndexerAction(idx: IndexerRow): boolean {
  if (idx.installed) {
    return idx.managed;
  }
  return idx.installable;
}

function getIndexerActionTitle(idx: IndexerRow): string {
  if (idx.installed && !idx.managed) {
    return `${idx.actionIndexer} is installed outside SCIP-IO; remove it from PATH manually.`;
  }
  if (!idx.installed && !idx.installable) {
    return 'Automatic install is not supported for this indexer.';
  }
  if (idx.coveredBy) {
    return idx.installed
      ? `Uninstall ${idx.actionIndexer} to remove ${idx.name} support`
      : `Install ${idx.actionIndexer} to enable ${idx.name}`;
  }
  return idx.installed ? `Uninstall ${idx.name}` : `Install ${idx.name}`;
}

function getIndexerActionLabel(idx: IndexerRow, verb: string): string {
  if (idx.coveredBy) {
    return `${verb} ${idx.actionIndexer} for ${idx.name}`;
  }
  return `${verb} ${idx.name}`;
}

async function handleInstallIndexer(idx: IndexerRow): Promise<void> {
  await runIndexerAction(idx, async () => {
    addLog('info', `${getIndexerActionLabel(idx, 'Installing')}...`);
    await installIndexer(idx.actionIndexer);
    addLog('success', `${idx.actionIndexer} installed`);
  });
}

async function handleUninstallIndexer(idx: IndexerRow): Promise<void> {
  await runIndexerAction(idx, async () => {
    addLog('info', `${getIndexerActionLabel(idx, 'Uninstalling')}...`);
    await uninstallIndexer(idx.actionIndexer);
    addLog('success', `${idx.actionIndexer} uninstalled`);
  });
}

async function runIndexerAction(idx: IndexerRow, action: () => Promise<void>): Promise<void> {
  pendingIndexerActions.add(idx.actionIndexer);
  refreshIndexerRows();
  try {
    await action();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    addLog('error', `${idx.name} action failed: ${message}`);
  } finally {
    pendingIndexerActions.delete(idx.actionIndexer);
    await handleFetchIndexers();
    refreshIndexerRows();
  }
}

function refreshIndexerRows(): void {
  store.setState({ indexers: [...store.getState().indexers] });
}

async function handleIndexAll() {
  const state = store.getState();
  const selectedLangs = state.languages.filter((l) => l.selected).map((l) => l.name);

  if (selectedLangs.length === 0) {
    addLog('warning', 'No languages selected for indexing');
    return;
  }

  store.setState({
    isIndexing: true,
    screen: 'indexing',
    pipelineStep: 'detect',
    overallProgress: 0,
    logs: [],
    indexerProgress: new Map(
      selectedLangs.map((lang) => [
        lang,
        { language: lang, status: 'queued' as const, progress: 0, message: 'Queued' },
      ])
    ),
  });

  addLog('info', `Starting indexing for: ${selectedLangs.join(', ')}`);

  try {
    await startIndexing(state.projectPath, selectedLangs, state.settings.outputFile);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    addLog('error', `Indexing failed: ${message}`);
    store.setState({ isIndexing: false });
  } finally {
    await handleFetchIndexers();
  }
}

// ----- Utilities -----

function escapeHtml(str: string): string {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}
