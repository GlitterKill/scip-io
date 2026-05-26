import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export interface LanguageInfo {
  name: string;
  kind: string;
  evidence: string;
}

export interface IndexerStatusInfo {
  name: string;
  language: string;
  version: string;
  binary_name: string;
  github_repo: string;
  installed: boolean;
  installable: boolean;
  managed: boolean;
  installed_path: string | null;
  action_indexer: string;
  covered_by: string | null;
}

export interface UpdateInfo {
  name: string;
  language: string;
  current_version: string;
  latest_version: string;
  update_available: boolean;
  installed: boolean;
  managed: boolean;
  action_indexer: string;
  error: string | null;
}

export interface ValidationResult {
  valid: boolean;
  errors: { kind: string; message: string }[];
  warnings: string[];
  stats: { documents: number; symbols: number; occurrences: number; languages: string[] } | null;
}

export interface ProgressEvent {
  [key: string]: unknown;
}

export async function detectLanguages(path: string): Promise<LanguageInfo[]> {
  return invoke('detect_languages', { path });
}

export async function startIndexing(
  path: string,
  languages: string[],
  output: string,
  includeAdditionalConfigs: boolean
): Promise<void> {
  return invoke('start_indexing', { path, languages, output, includeAdditionalConfigs });
}

export async function cancelIndexing(): Promise<void> {
  return invoke('cancel_indexing');
}

export async function getIndexerStatus(): Promise<IndexerStatusInfo[]> {
  return invoke('get_indexer_status');
}

export async function installIndexer(indexer: string): Promise<IndexerStatusInfo> {
  return invoke('install_indexer', { indexer });
}

export async function uninstallIndexer(indexer: string): Promise<IndexerStatusInfo> {
  return invoke('uninstall_indexer', { indexer });
}

export async function updateIndexer(indexer: string, version: string): Promise<IndexerStatusInfo> {
  return invoke('update_indexer', { indexer, version });
}

export async function getConfig(path: string): Promise<unknown> {
  return invoke('get_config', { path });
}

export async function saveConfig(path: string, config: unknown): Promise<void> {
  return invoke('save_config', { path, config });
}

export async function cleanCache(language?: string): Promise<string> {
  return invoke('clean_cache', { language: language || null });
}

export async function validateIndex(path: string): Promise<ValidationResult> {
  return invoke('validate_index', { path });
}

export async function checkUpdates(): Promise<UpdateInfo[]> {
  return invoke('check_updates');
}

export function onProgress(callback: (event: ProgressEvent) => void): Promise<() => void> {
  return listen('progress', (event) => {
    callback(event.payload as ProgressEvent);
  }).then(unlisten => unlisten);
}
