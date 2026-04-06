type Listener<T> = (state: T) => void;

export class Store<T> {
  private state: T;
  private listeners: Set<Listener<T>> = new Set();

  constructor(initial: T) {
    this.state = initial;
  }

  getState(): T {
    return this.state;
  }

  setState(partial: Partial<T>) {
    this.state = { ...this.state, ...partial };
    this.listeners.forEach((fn) => fn(this.state));
  }

  subscribe(fn: Listener<T>): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
}

export type Screen = 'dashboard' | 'indexing' | 'results' | 'settings';

export interface LogEntry {
  timestamp: string;
  level: 'info' | 'success' | 'error' | 'warning';
  message: string;
}

export interface IndexerProgress {
  language: string;
  status: 'queued' | 'downloading' | 'running' | 'done' | 'failed';
  progress: number;
  message: string;
  duration?: number;
  symbols?: number;
  files?: number;
}

export interface IndexingResult {
  output: string;
  totalFiles: number;
  totalSymbols: number;
  totalDuration: number;
  languages: Array<{
    name: string;
    files: number;
    symbols: number;
    duration: number;
  }>;
  outputSize: number;
}

export interface AppState {
  screen: Screen;
  projectPath: string;
  languages: Array<{ name: string; evidence: string; selected: boolean }>;
  indexers: Array<{
    name: string;
    language: string;
    version: string;
    installed: boolean;
    installedPath: string | null;
  }>;
  isIndexing: boolean;
  indexerProgress: Map<string, IndexerProgress>;
  pipelineStep: 'detect' | 'download' | 'index' | 'merge' | 'done';
  overallProgress: number;
  logs: LogEntry[];
  results: IndexingResult | null;
  settings: {
    parallel: boolean;
    timeout: number;
    outputFile: string;
    cacheDir: string;
  };
}

export const store = new Store<AppState>({
  screen: 'dashboard',
  projectPath: '.',
  languages: [],
  indexers: [],
  isIndexing: false,
  indexerProgress: new Map(),
  pipelineStep: 'detect',
  overallProgress: 0,
  logs: [],
  results: null,
  settings: {
    parallel: true,
    timeout: 300,
    outputFile: 'index.scip',
    cacheDir: '',
  },
});

export function addLog(level: LogEntry['level'], message: string) {
  const state = store.getState();
  const entry: LogEntry = {
    timestamp: new Date().toLocaleTimeString(),
    level,
    message,
  };
  store.setState({ logs: [...state.logs, entry] });
}
