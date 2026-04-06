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
  installed_path: string | null;
}

export interface UpdateInfo {
  name: string;
  language: string;
  current_version: string;
  latest_version: string;
  update_available: boolean;
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

export async function startIndexing(path: string, languages: string[], output: string): Promise<void> {
  return invoke('start_indexing', { path, languages, output });
}

export async function cancelIndexing(): Promise<void> {
  return invoke('cancel_indexing');
}

export async function getIndexerStatus(): Promise<IndexerStatusInfo[]> {
  return invoke('get_indexer_status');
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
