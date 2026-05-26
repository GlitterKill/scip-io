use std::collections::BTreeSet;

use anyhow::{Result, anyhow};

use scip_io_core::indexer::IndexerEntry;
use scip_io_core::indexer::registry::REGISTRY;

/// Resolve user-facing language/indexer names to the concrete indexer that
/// should be installed, removed, or updated.
pub fn action_entry_for_target(target: &str) -> Result<&'static IndexerEntry> {
    let entry = find_entry(target)
        .ok_or_else(|| anyhow!("unknown SCIP indexer or language '{}'", target))?;
    Ok(REGISTRY.action_entry_for(entry).unwrap_or(entry))
}

/// Return install/update action entries once even when multiple language rows
/// share the same underlying indexer binary.
pub fn unique_action_entries() -> Vec<&'static IndexerEntry> {
    let mut seen = BTreeSet::new();
    REGISTRY
        .all()
        .iter()
        .filter_map(|entry| {
            let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
            let key = action_entry.indexer_name.to_ascii_lowercase();
            seen.insert(key).then_some(action_entry)
        })
        .collect()
}

fn find_entry(target: &str) -> Option<&'static IndexerEntry> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }

    REGISTRY.all().iter().find(|entry| {
        entry.indexer_name.eq_ignore_ascii_case(target)
            || entry.binary_name().eq_ignore_ascii_case(target)
            || entry.language_name().eq_ignore_ascii_case(target)
    })
}

#[cfg(test)]
mod tests {
    use super::action_entry_for_target;

    #[test]
    fn resolves_language_to_indexer() {
        let entry = action_entry_for_target("python").unwrap();
        assert_eq!(entry.indexer_name, "scip-python");
    }

    #[test]
    fn resolves_kotlin_to_java_action_indexer() {
        let entry = action_entry_for_target("kotlin").unwrap();
        assert_eq!(entry.indexer_name, "scip-java");
    }
}
