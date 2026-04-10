use craft::project::ResolvedAnalysis;
use kernc_driver::SourceOverrides;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub(super) struct DirtyDocumentsSnapshot {
    pub(super) overrides: SourceOverrides,
    pub(super) hashed_overrides: Vec<(PathBuf, u64)>,
}

impl DirtyDocumentsSnapshot {
    pub(super) fn is_clean(&self) -> bool {
        self.hashed_overrides.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.hashed_overrides.len()
    }

    pub(super) fn remap_for(&self, aliases: &std::collections::BTreeMap<PathBuf, PathBuf>) -> Self {
        if aliases.is_empty() || self.overrides.is_empty() {
            return self.clone();
        }

        let mut overrides = self.overrides.clone();
        for (source_path, generated_path) in aliases {
            let normalized_source = super::normalize_path(source_path);
            let normalized_generated = super::normalize_path(generated_path);
            if overrides.contains_key(&normalized_generated) {
                continue;
            }
            let Some(source) = overrides.get(&normalized_source).cloned() else {
                continue;
            };
            overrides.insert(normalized_generated, source);
        }

        let mut hashed_overrides = overrides
            .iter()
            .map(|(path, text)| (super::normalize_path(path), hash_source_text(text)))
            .collect::<Vec<_>>();
        hashed_overrides.sort();

        Self {
            overrides,
            hashed_overrides,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct AnalysisCacheKey {
    input_file: PathBuf,
    root_module_name: Option<String>,
    target_triple: String,
    custom_defines: Vec<(String, String)>,
    module_aliases: Vec<(String, String)>,
    module_interface_aliases: Vec<(String, String)>,
    source_overrides: Vec<(PathBuf, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct AnalysisCacheFamilyKey {
    input_file: PathBuf,
    root_module_name: Option<String>,
    target_triple: String,
    custom_defines: Vec<(String, String)>,
    module_aliases: Vec<(String, String)>,
    module_interface_aliases: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SemanticTokensCacheKey {
    pub(super) analysis: AnalysisCacheKey,
    pub(super) target_path: PathBuf,
    pub(super) document_version: i64,
}

impl AnalysisCacheKey {
    #[cfg(test)]
    pub(super) fn from_resolved(
        resolved: &ResolvedAnalysis,
        source_overrides: &SourceOverrides,
    ) -> Self {
        let mut hashed_overrides = source_overrides
            .iter()
            .map(|(path, text)| (super::normalize_path(path), hash_source_text(text)))
            .collect::<Vec<_>>();
        hashed_overrides.sort();
        Self::from_resolved_hashed(resolved, hashed_overrides)
    }

    pub(super) fn from_resolved_dirty_snapshot(
        resolved: &ResolvedAnalysis,
        dirty_documents: &DirtyDocumentsSnapshot,
    ) -> Self {
        Self::from_resolved_hashed(resolved, dirty_documents.hashed_overrides.clone())
    }

    pub(super) fn clean(resolved: &ResolvedAnalysis) -> Self {
        Self::from_resolved_hashed(resolved, Vec::new())
    }

    fn from_resolved_hashed(
        resolved: &ResolvedAnalysis,
        source_overrides: Vec<(PathBuf, u64)>,
    ) -> Self {
        let mut custom_defines = resolved
            .compile_options
            .custom_defines
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        custom_defines.sort();

        let mut module_aliases = resolved
            .compile_options
            .module_aliases
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        module_aliases.sort();

        let mut module_interface_aliases = resolved
            .compile_options
            .module_interface_aliases
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        module_interface_aliases.sort();

        Self {
            input_file: super::normalize_path(&resolved.input_file),
            root_module_name: resolved.compile_options.root_module_name.clone(),
            target_triple: resolved.compile_options.target.triple.to_string(),
            custom_defines,
            module_aliases,
            module_interface_aliases,
            source_overrides,
        }
    }

    pub(super) fn is_clean(&self) -> bool {
        self.source_overrides.is_empty()
    }

    pub(super) fn family(&self) -> AnalysisCacheFamilyKey {
        AnalysisCacheFamilyKey {
            input_file: self.input_file.clone(),
            root_module_name: self.root_module_name.clone(),
            target_triple: self.target_triple.clone(),
            custom_defines: self.custom_defines.clone(),
            module_aliases: self.module_aliases.clone(),
            module_interface_aliases: self.module_interface_aliases.clone(),
        }
    }
}

pub(super) fn hash_source_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
