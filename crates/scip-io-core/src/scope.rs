use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::IndexScope;
use crate::config_discovery::{
    discover_additional_config_roots, supported_additional_config_languages,
};
use crate::detect::{
    LanguageScanOptions, discover_indexable_project_roots, scan_languages_with_options,
};

/// One project root selected for indexing and the child roots it should not own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIndexingRoot {
    pub root: PathBuf,
    pub excluded_roots: Vec<PathBuf>,
    pub owned_child_prefixes: Vec<String>,
}

/// Inputs for resolving which project roots an index operation should schedule.
#[derive(Debug, Clone)]
pub struct IndexScopeResolution<'a> {
    pub base_path: &'a Path,
    pub scope: IndexScope,
    pub explicit_roots: &'a [PathBuf],
    pub all_roots: bool,
    pub include_additional_configs: bool,
    pub language_filters: &'a [String],
}

/// Resolve project roots once so CLI and GUI scope behavior cannot drift.
pub fn resolve_indexing_roots(
    options: IndexScopeResolution<'_>,
) -> Result<Vec<ResolvedIndexingRoot>> {
    let roots = resolve_root_paths(&options)?;
    Ok(roots
        .iter()
        .map(|root| {
            let excluded_roots = roots
                .iter()
                .filter(|candidate| *candidate != root && candidate.starts_with(root))
                .cloned()
                .collect::<Vec<_>>();
            let owned_child_prefixes = child_prefixes_for_project_root(root, &excluded_roots);
            ResolvedIndexingRoot {
                root: root.clone(),
                excluded_roots,
                owned_child_prefixes,
            }
        })
        .collect())
}

fn resolve_root_paths(options: &IndexScopeResolution<'_>) -> Result<Vec<PathBuf>> {
    if !options.explicit_roots.is_empty() {
        return resolve_explicit_roots(options.base_path, options.explicit_roots);
    }

    if options.scope == IndexScope::RepoTree && !options.all_roots {
        return Ok(vec![options.base_path.to_path_buf()]);
    }

    let mut roots = discover_indexable_project_roots(options.base_path)?;
    if !options.all_roots {
        roots.push(options.base_path.to_path_buf());
    }
    if options.include_additional_configs
        && has_allowed_additional_config_language(options.language_filters)
    {
        roots.extend(discover_additional_config_roots(options.base_path)?);
    }
    roots.sort();
    roots.dedup();
    roots = filter_roots_by_language(options.language_filters, roots)?;
    if roots.is_empty() {
        bail!(
            "No language config roots found under {}",
            options.base_path.display()
        );
    }
    Ok(roots)
}

fn resolve_explicit_roots(base_path: &Path, explicit_roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let canonical_base = base_path
        .canonicalize()
        .with_context(|| format!("Invalid base path: {}", base_path.display()))?;
    let mut seen = HashSet::new();
    let mut roots = Vec::new();
    for root in explicit_roots {
        let candidate = if root.is_absolute() {
            root.clone()
        } else {
            base_path.join(root)
        };
        let candidate = candidate
            .canonicalize()
            .with_context(|| format!("Invalid project root: {}", candidate.display()))?;
        if !candidate.starts_with(&canonical_base) {
            bail!(
                "Project root {} is outside base path {}",
                candidate.display(),
                base_path.display()
            );
        }
        if seen.insert(candidate.clone()) {
            roots.push(candidate);
        }
    }
    Ok(roots)
}

fn filter_roots_by_language(
    language_filters: &[String],
    roots: Vec<PathBuf>,
) -> Result<Vec<PathBuf>> {
    if language_filters.is_empty() {
        return Ok(roots);
    }

    let mut filtered = Vec::new();
    for root in roots {
        let detected = scan_languages_with_options(
            &root,
            LanguageScanOptions {
                max_depth: Some(1),
                excluded_roots: Vec::new(),
            },
        )?;
        if detected
            .iter()
            .any(|language| language_filter_allows(language_filters, language.kind))
        {
            filtered.push(root);
        }
    }
    Ok(filtered)
}

fn child_prefixes_for_project_root(root: &Path, child_roots: &[PathBuf]) -> Vec<String> {
    let mut prefixes = child_roots
        .iter()
        .filter_map(|child_root| child_root.strip_prefix(root).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

fn has_allowed_additional_config_language(language_filters: &[String]) -> bool {
    supported_additional_config_languages()
        .iter()
        .any(|&kind| language_filter_allows(language_filters, kind))
}

fn language_filter_allows(
    language_filters: &[String],
    kind: crate::detect::languages::LanguageKind,
) -> bool {
    language_filters.is_empty()
        || language_filters
            .iter()
            .any(|name| name.eq_ignore_ascii_case(kind.name()))
}
