import { store, addLog } from '../state/store.js';
import { getConfig, saveConfig, cleanCache, checkUpdates } from '../bridge/tauri.js';

// Known indexers for the override section
const KNOWN_INDEXERS = [
  { name: 'scip-typescript', language: 'TypeScript/JavaScript', defaultBinary: 'scip-typescript' },
  { name: 'rust-analyzer', language: 'Rust', defaultBinary: 'rust-analyzer' },
  { name: 'scip-go', language: 'Go', defaultBinary: 'scip-go' },
  { name: 'scip-java', language: 'Java', defaultBinary: 'scip-java' },
  { name: 'scip-python', language: 'Python', defaultBinary: 'scip-python' },
  { name: 'scip-dotnet', language: 'C#', defaultBinary: 'scip-dotnet' },
  { name: 'scip-ruby', language: 'Ruby', defaultBinary: 'scip-ruby' },
  { name: 'scip-kotlin', language: 'Kotlin', defaultBinary: 'scip-kotlin' },
  { name: 'scip-clang', language: 'C/C++', defaultBinary: 'scip-clang' },
];

interface IndexerOverride {
  name: string;
  binaryPath: string;
  args: string;
  expanded: boolean;
}

let overrides: IndexerOverride[] = KNOWN_INDEXERS.map((idx) => ({
  name: idx.name,
  binaryPath: '',
  args: '',
  expanded: false,
}));

export function renderSettings(container: HTMLElement): void {
  const wrapper = document.createElement('div');
  wrapper.className = 'content-wrapper section';
  container.appendChild(wrapper);

  // Header
  const header = document.createElement('div');
  header.className = 'flex items-center justify-between mb-lg animate-fade-in';
  header.innerHTML = `
    <div>
      <h2 class="text-xl">Settings</h2>
      <p class="text-sm text-secondary mt-xs">Configure SCIP-IO behavior and indexer overrides.</p>
    </div>
  `;
  wrapper.appendChild(header);

  // General settings
  wrapper.appendChild(renderGeneralSettings());

  // Indexer overrides
  wrapper.appendChild(renderIndexerOverrides());

  // Updates section
  wrapper.appendChild(renderUpdatesSection());

  // Save/Reset buttons
  wrapper.appendChild(renderSaveActions());

  // Load config
  loadConfig();
}

// ----- General Settings -----

function renderGeneralSettings(): HTMLElement {
  const panel = document.createElement('div');
  panel.className = 'panel animate-fade-in';

  const state = store.getState();
  const settings = state.settings;

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">General</span>
    <div class="section-header__line"></div>
  `;
  panel.appendChild(sectionHeader);

  const grid = document.createElement('div');
  grid.className = 'grid grid-2 gap-lg';

  // Parallel toggle
  const parallelField = document.createElement('div');
  parallelField.className = 'form-field';
  parallelField.innerHTML = `
    <label class="form-label">Parallel Indexing</label>
    <div class="flex items-center gap-sm">
      <label class="chip ${settings.parallel ? 'chip--selected' : ''}" id="setting-parallel-chip">
        <span class="chip__checkbox"></span>
        <span class="chip__label">Enabled</span>
      </label>
      <span class="form-hint">Run indexers concurrently</span>
    </div>
  `;
  grid.appendChild(parallelField);

  // Timeout
  const timeoutField = document.createElement('div');
  timeoutField.className = 'form-field';
  timeoutField.innerHTML = `
    <label class="form-label">Timeout (seconds)</label>
    <input class="input" type="number" id="setting-timeout" value="${settings.timeout}" min="30" max="3600" placeholder="300" />
    <span class="form-hint">Maximum time per indexer</span>
  `;
  grid.appendChild(timeoutField);

  // Output file
  const outputField = document.createElement('div');
  outputField.className = 'form-field';
  outputField.innerHTML = `
    <label class="form-label">Output File</label>
    <input class="input" type="text" id="setting-output" value="${escapeAttr(settings.outputFile)}" placeholder="index.scip" />
    <span class="form-hint">Output SCIP index filename</span>
  `;
  grid.appendChild(outputField);

  // Cache directory
  const cacheField = document.createElement('div');
  cacheField.className = 'form-field';
  cacheField.innerHTML = `
    <label class="form-label">Cache Directory</label>
    <div class="flex gap-sm">
      <input class="input flex-1" type="text" id="setting-cache-dir" value="${escapeAttr(settings.cacheDir)}" placeholder="(default system cache)" />
      <button class="btn btn--ghost btn--sm" id="btn-browse-cache">Browse</button>
    </div>
    <span class="form-hint">Where indexer binaries are stored</span>
  `;
  grid.appendChild(cacheField);

  panel.appendChild(grid);

  // Event bindings after render
  setTimeout(() => {
    const parallelChip = document.getElementById('setting-parallel-chip');
    if (parallelChip) {
      parallelChip.addEventListener('click', () => {
        const current = store.getState().settings;
        store.setState({ settings: { ...current, parallel: !current.parallel } });
        parallelChip.classList.toggle('chip--selected');
      });
    }

    const browseCache = document.getElementById('btn-browse-cache');
    if (browseCache) {
      browseCache.addEventListener('click', handleBrowseCache);
    }
  }, 0);

  return panel;
}

// ----- Indexer Overrides -----

function renderIndexerOverrides(): HTMLElement {
  const panel = document.createElement('div');
  panel.className = 'panel animate-fade-in stagger-2';

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">Indexer Overrides</span>
    <span class="section-header__count">${KNOWN_INDEXERS.length}</span>
    <div class="section-header__line"></div>
  `;
  panel.appendChild(sectionHeader);

  const hint = document.createElement('p');
  hint.className = 'text-sm text-muted mb-lg';
  hint.textContent = 'Override binary paths or add extra arguments for specific indexers. Leave blank to use defaults.';
  panel.appendChild(hint);

  const list = document.createElement('div');
  list.className = 'flex flex-col gap-sm';
  list.id = 'indexer-overrides-list';

  overrides.forEach((override, index) => {
    const indexer = KNOWN_INDEXERS[index];
    list.appendChild(createOverrideItem(override, indexer, index));
  });

  panel.appendChild(list);

  return panel;
}

function createOverrideItem(
  override: IndexerOverride,
  indexer: typeof KNOWN_INDEXERS[0],
  index: number
): HTMLElement {
  const item = document.createElement('div');
  item.className = 'panel panel--inset panel--compact';

  // Clickable header row
  const headerRow = document.createElement('div');
  headerRow.className = 'flex items-center justify-between cursor-pointer';
  headerRow.innerHTML = `
    <div class="flex items-center gap-sm">
      <span class="text-sm font-medium text-primary">${escapeHtml(indexer.name)}</span>
      <span class="text-xs text-muted">${escapeHtml(indexer.language)}</span>
    </div>
    <div class="flex items-center gap-sm">
      ${override.binaryPath || override.args ? '<span class="badge badge--outdated"><span class="badge__dot"></span> Custom</span>' : '<span class="text-xs text-muted">Default</span>'}
      <svg class="override-chevron transition-transform" width="12" height="12" viewBox="0 0 12 12" style="transform: rotate(${override.expanded ? '180' : '0'}deg);">
        <path d="M2 4l4 4 4-4" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/>
      </svg>
    </div>
  `;

  headerRow.addEventListener('click', () => {
    overrides[index].expanded = !overrides[index].expanded;
    const details = item.querySelector('.override-details') as HTMLElement | null;
    const chevron = item.querySelector('.override-chevron') as SVGElement | null;
    if (details) {
      details.style.display = overrides[index].expanded ? 'block' : 'none';
    }
    if (chevron) {
      chevron.style.transform = overrides[index].expanded ? 'rotate(180deg)' : 'rotate(0deg)';
    }
  });

  item.appendChild(headerRow);

  // Expandable details
  const details = document.createElement('div');
  details.className = 'override-details mt-md';
  details.style.display = override.expanded ? 'block' : 'none';

  details.innerHTML = `
    <div class="grid grid-2 gap-md">
      <div class="form-field">
        <label class="form-label">Binary Path</label>
        <input class="input" type="text" data-override-idx="${index}" data-field="binaryPath"
               value="${escapeAttr(override.binaryPath)}" placeholder="${escapeAttr(indexer.defaultBinary)}" />
      </div>
      <div class="form-field">
        <label class="form-label">Extra Arguments</label>
        <input class="input" type="text" data-override-idx="${index}" data-field="args"
               value="${escapeAttr(override.args)}" placeholder="--verbose --threads 4" />
      </div>
    </div>
  `;

  // Bind input changes
  details.querySelectorAll('input').forEach((input) => {
    input.addEventListener('change', () => {
      const idx = parseInt(input.getAttribute('data-override-idx') || '0', 10);
      const field = input.getAttribute('data-field') as 'binaryPath' | 'args';
      if (overrides[idx]) {
        overrides[idx][field] = input.value;
      }
    });
  });

  item.appendChild(details);

  return item;
}

// ----- Updates Section -----

function renderUpdatesSection(): HTMLElement {
  const panel = document.createElement('div');
  panel.className = 'panel animate-fade-in stagger-3';

  const sectionHeader = document.createElement('div');
  sectionHeader.className = 'section-header';
  sectionHeader.innerHTML = `
    <span class="section-header__title">Updates</span>
    <div class="section-header__line"></div>
  `;
  panel.appendChild(sectionHeader);

  const body = document.createElement('div');
  body.className = 'flex items-center gap-md';

  const checkBtn = document.createElement('button');
  checkBtn.className = 'btn btn--secondary';
  checkBtn.textContent = 'Check for Updates';
  checkBtn.addEventListener('click', handleCheckUpdates);
  body.appendChild(checkBtn);

  const cleanBtn = document.createElement('button');
  cleanBtn.className = 'btn btn--ghost';
  cleanBtn.textContent = 'Clean Cache';
  cleanBtn.addEventListener('click', handleCleanCache);
  body.appendChild(cleanBtn);

  // Update results area
  const resultsArea = document.createElement('div');
  resultsArea.id = 'update-results';
  resultsArea.className = 'flex-1';
  body.appendChild(resultsArea);

  panel.appendChild(body);

  return panel;
}

// ----- Save/Reset Actions -----

function renderSaveActions(): HTMLElement {
  const actions = document.createElement('div');
  actions.className = 'flex justify-end gap-sm mt-lg animate-fade-in stagger-4';

  const resetBtn = document.createElement('button');
  resetBtn.className = 'btn btn--ghost';
  resetBtn.textContent = 'Reset to Defaults';
  resetBtn.addEventListener('click', handleReset);
  actions.appendChild(resetBtn);

  const saveBtn = document.createElement('button');
  saveBtn.className = 'btn btn--primary-filled';
  saveBtn.textContent = 'Save Settings';
  saveBtn.addEventListener('click', handleSave);
  actions.appendChild(saveBtn);

  return actions;
}

// ----- Handlers -----

async function loadConfig() {
  try {
    const config = await getConfig(store.getState().projectPath);
    if (config && typeof config === 'object') {
      const c = config as Record<string, unknown>;
      const current = store.getState().settings;
      store.setState({
        settings: {
          parallel: typeof c.parallel === 'boolean' ? c.parallel : current.parallel,
          timeout: typeof c.timeout === 'number' ? c.timeout : current.timeout,
          outputFile: typeof c.output === 'string' ? c.output : current.outputFile,
          cacheDir: typeof c.cache_dir === 'string' ? c.cache_dir : current.cacheDir,
        },
      });
    }
  } catch {
    // Config not available; using defaults
  }
}

async function handleSave() {
  // Read current form values
  const timeoutInput = document.getElementById('setting-timeout') as HTMLInputElement | null;
  const outputInput = document.getElementById('setting-output') as HTMLInputElement | null;
  const cacheInput = document.getElementById('setting-cache-dir') as HTMLInputElement | null;

  const current = store.getState().settings;
  const settings = {
    parallel: current.parallel,
    timeout: timeoutInput ? parseInt(timeoutInput.value, 10) || 300 : current.timeout,
    outputFile: outputInput ? outputInput.value || 'index.scip' : current.outputFile,
    cacheDir: cacheInput ? cacheInput.value : current.cacheDir,
  };

  store.setState({ settings });

  // Build config object for the backend
  const config = {
    parallel: settings.parallel,
    timeout: settings.timeout,
    output: settings.outputFile,
    cache_dir: settings.cacheDir || undefined,
    overrides: overrides
      .filter((o) => o.binaryPath || o.args)
      .map((o) => ({
        name: o.name,
        binary_path: o.binaryPath || undefined,
        extra_args: o.args ? o.args.split(' ').filter(Boolean) : undefined,
      })),
  };

  try {
    await saveConfig(store.getState().projectPath, config);
    addLog('success', 'Settings saved successfully');
    showNotification('Settings saved', 'success');
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    addLog('error', `Failed to save settings: ${message}`);
    showNotification('Failed to save settings', 'error');
  }
}

function handleReset() {
  store.setState({
    settings: {
      parallel: true,
      timeout: 300,
      outputFile: 'index.scip',
      cacheDir: '',
    },
  });

  overrides = KNOWN_INDEXERS.map((idx) => ({
    name: idx.name,
    binaryPath: '',
    args: '',
    expanded: false,
  }));

  // Update form fields
  const timeoutInput = document.getElementById('setting-timeout') as HTMLInputElement | null;
  const outputInput = document.getElementById('setting-output') as HTMLInputElement | null;
  const cacheInput = document.getElementById('setting-cache-dir') as HTMLInputElement | null;
  const parallelChip = document.getElementById('setting-parallel-chip');

  if (timeoutInput) timeoutInput.value = '300';
  if (outputInput) outputInput.value = 'index.scip';
  if (cacheInput) cacheInput.value = '';
  if (parallelChip) parallelChip.classList.add('chip--selected');

  // Re-render overrides
  const overridesList = document.getElementById('indexer-overrides-list');
  if (overridesList) {
    overridesList.innerHTML = '';
    overrides.forEach((override, index) => {
      overridesList.appendChild(createOverrideItem(override, KNOWN_INDEXERS[index], index));
    });
  }

  addLog('info', 'Settings reset to defaults');
  showNotification('Settings reset', 'info');
}

async function handleBrowseCache() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const selected = await open({ directory: true, multiple: false, title: 'Select Cache Directory' });
    if (selected && typeof selected === 'string') {
      const input = document.getElementById('setting-cache-dir') as HTMLInputElement | null;
      if (input) input.value = selected;
    }
  } catch {
    const path = prompt('Enter cache directory path:');
    if (path) {
      const input = document.getElementById('setting-cache-dir') as HTMLInputElement | null;
      if (input) input.value = path;
    }
  }
}

async function handleCheckUpdates() {
  const resultsArea = document.getElementById('update-results');
  if (resultsArea) {
    resultsArea.innerHTML = '<span class="spinner spinner--sm"></span> <span class="text-sm text-muted ml-sm">Checking...</span>';
  }

  try {
    const updates = await checkUpdates();
    if (resultsArea) {
      if (updates.length === 0) {
        resultsArea.innerHTML = '<span class="text-sm text-success">All indexers are up to date.</span>';
      } else {
        const available = updates.filter((u) => u.update_available);
        if (available.length === 0) {
          resultsArea.innerHTML = '<span class="text-sm text-success">All indexers are up to date.</span>';
        } else {
          resultsArea.innerHTML = `<span class="text-sm text-warning">${available.length} update(s) available: ${available.map((u) => u.name).join(', ')}</span>`;
        }
      }
    }
    addLog('info', `Update check complete: ${updates.filter((u) => u.update_available).length} update(s) available`);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    if (resultsArea) {
      resultsArea.innerHTML = `<span class="text-sm text-error">Check failed: ${escapeHtml(message)}</span>`;
    }
    addLog('error', `Update check failed: ${message}`);
  }
}

async function handleCleanCache() {
  try {
    const result = await cleanCache();
    addLog('success', `Cache cleaned: ${result}`);
    showNotification('Cache cleaned', 'success');
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    addLog('error', `Failed to clean cache: ${message}`);
    showNotification('Failed to clean cache', 'error');
  }
}

// ----- Simple Notification Toast -----

function showNotification(message: string, type: 'success' | 'error' | 'info') {
  const existing = document.querySelector('.notification-toast');
  if (existing) existing.remove();

  const toast = document.createElement('div');
  toast.className = 'notification-toast fixed animate-slide-in-right';
  toast.style.bottom = 'var(--space-xl)';
  toast.style.right = 'var(--space-xl)';
  toast.style.zIndex = '1000';

  const colorClass = type === 'success' ? 'text-success' : type === 'error' ? 'text-error' : 'text-cyan';
  const bgClass = type === 'success' ? 'color-success-dim' : type === 'error' ? 'color-error-dim' : 'accent-cyan-dim';

  toast.innerHTML = `
    <div class="panel panel--compact flex items-center gap-sm" style="background: var(--${bgClass}); border-color: var(--${type === 'success' ? 'color-success' : type === 'error' ? 'color-error' : 'accent-cyan'});">
      <span class="${colorClass} text-sm font-medium">${escapeHtml(message)}</span>
    </div>
  `;

  document.body.appendChild(toast);

  setTimeout(() => {
    toast.classList.remove('animate-slide-in-right');
    toast.classList.add('animate-slide-out-right');
    setTimeout(() => toast.remove(), 300);
  }, 3000);
}

// ----- Utilities -----

function escapeHtml(str: string): string {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

function escapeAttr(str: string): string {
  return str.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}
