use super::{
    ActionPlan, ArtifactKind, BuildPlan, BuildUnit, CompileAction, CompileSourceInput, LinkAction,
    SourceRootBinding, StagedAction, artifact_path, metadata_path, object_path,
    resolve_compile_source_input, resolve_staged_action,
};
use crate::plan::TargetKind;
use crate::script;
use std::collections::BTreeMap;
use std::path::Path;

impl ActionPlan {
    pub fn compile_count(&self) -> usize {
        self.compile_actions.len()
    }

    pub fn link_count(&self) -> usize {
        self.link_actions.len()
    }

    #[cfg(test)]
    pub fn artifact_output_nodes_for_link_action<'a>(
        &'a self,
        action: &LinkAction,
    ) -> Vec<&'a StagedAction> {
        collect_build_nodes(
            self.build_nodes.as_slice(),
            action.artifact_outputs.as_slice(),
        )
    }
}

impl SourceRootBinding {
    pub fn display_path(&self) -> &str {
        match self {
            Self::PackagePath(path) | Self::AbsolutePath(path) => path.as_str(),
            Self::BuildOutput { path, .. } => path.as_str(),
        }
    }
}

impl CompileSourceInput {
    pub fn path(&self) -> &Path {
        match self {
            Self::PackagePath(path) | Self::AbsolutePath(path) => path.as_path(),
            Self::BuildOutput { path, .. } => path.as_path(),
        }
    }
}

impl CompileAction {
    pub fn source_path(&self) -> &Path {
        self.source_input.path()
    }

    pub fn required_source_path(&self) -> Option<&Path> {
        match &self.source_input {
            CompileSourceInput::BuildOutput { path, .. } => Some(path.as_path()),
            CompileSourceInput::PackagePath(_) | CompileSourceInput::AbsolutePath(_) => None,
        }
    }
}

impl BuildPlan {
    pub fn filtered_target_kinds(&self, keep: &[TargetKind]) -> Self {
        let mut filtered = self.clone();
        for package in &mut filtered.packages {
            package
                .units
                .retain(|unit| keep.contains(&unit.target_kind));
        }
        filtered
    }

    pub fn unit_count(&self) -> usize {
        self.packages
            .iter()
            .map(|package| package.units.len())
            .sum()
    }

    pub fn local_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.local_dependencies.len())
            .sum()
    }

    pub fn external_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.external_dependencies.len())
            .sum()
    }

    pub fn build_local_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .map(|package| package.build_local_dependencies.len())
            .sum()
    }

    pub fn build_external_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .map(|package| package.build_external_dependencies.len())
            .sum()
    }

    pub fn build_script_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|package| package.build_script.is_some())
            .count()
    }

    pub fn generated_file_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.generated_files.len())
            .sum()
    }

    pub fn staged_action_count(&self) -> usize {
        self.build_nodes.len()
    }

    pub fn compile_input_nodes_for_unit<'a>(&'a self, unit: &BuildUnit) -> Vec<&'a StagedAction> {
        collect_build_nodes(
            self.build_nodes.as_slice(),
            unit.build.compile_inputs.as_slice(),
        )
    }

    pub fn artifact_output_nodes_for_unit<'a>(&'a self, unit: &BuildUnit) -> Vec<&'a StagedAction> {
        collect_build_nodes(
            self.build_nodes.as_slice(),
            unit.build.artifact_outputs.as_slice(),
        )
    }

    pub fn link_system_lib_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.system_libs.len())
            .sum()
    }

    pub fn link_framework_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.frameworks.len())
            .sum()
    }

    pub fn link_search_path_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.search_paths.len())
            .sum()
    }

    pub fn link_arg_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.args.len())
            .sum()
    }

    pub fn derive_actions(&self, target: &script::ScriptTarget) -> ActionPlan {
        let host = script::host_target();
        self.derive_actions_with_targets(&host, target)
    }

    pub fn derive_actions_with_targets(
        &self,
        host: &script::ScriptTarget,
        target: &script::ScriptTarget,
    ) -> ActionPlan {
        let build_nodes = self
            .build_nodes
            .iter()
            .map(|action| resolve_staged_action(&self.workspace_root, action))
            .collect();
        let mut compile_actions = Vec::new();

        for package in &self.packages {
            let package_root = package
                .manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."));
            for unit in &package.units {
                compile_actions.push(CompileAction {
                    domain: unit.domain,
                    package_id: unit.package_id.clone(),
                    manifest_path: package.manifest_path.clone(),
                    target_kind: unit.target_kind,
                    target_name: unit.target_name.clone(),
                    artifact_name: unit.artifact_name.clone(),
                    source_input: resolve_compile_source_input(package_root, &unit.source_root),
                    metadata_path: (unit.target_kind == TargetKind::Lib).then(|| {
                        metadata_path(
                            &self.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                        )
                    }),
                    object_path: object_path(
                        &self.workspace_root,
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    artifact_path: artifact_path(
                        &self.workspace_root,
                        unit.domain.select_target(host, target),
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    profile: unit.profile.clone(),
                    cfg: unit.cfg.clone(),
                    define: unit.define.clone(),
                    compile_inputs: unit.build.compile_inputs.clone(),
                    local_dependencies: unit.local_dependencies.clone(),
                    external_dependencies: unit.external_dependencies.clone(),
                });
            }
        }

        let mut package_lib_objects = BTreeMap::new();
        for action in &compile_actions {
            if action.target_kind == TargetKind::Lib {
                package_lib_objects.insert(
                    (action.domain, action.package_id.clone()),
                    action.object_path.clone(),
                );
            }
        }

        let mut link_actions = Vec::new();
        for package in &self.packages {
            for unit in &package.units {
                if unit.artifact_kind != ArtifactKind::Executable {
                    continue;
                }

                link_actions.push(LinkAction {
                    domain: unit.domain,
                    package_id: unit.package_id.clone(),
                    manifest_path: package.manifest_path.clone(),
                    package_root_path: unit.package_root_path.clone(),
                    target_kind: unit.target_kind,
                    target_name: unit.target_name.clone(),
                    artifact_name: unit.artifact_name.clone(),
                    artifact_path: artifact_path(
                        &self.workspace_root,
                        unit.domain.select_target(host, target),
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    primary_object: object_path(
                        &self.workspace_root,
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    local_library_objects: unit
                        .local_dependencies
                        .iter()
                        .filter_map(|binding| {
                            package_lib_objects
                                .get(&(binding.domain, binding.package_id.clone()))
                                .cloned()
                        })
                        .collect(),
                    artifact_outputs: unit.build.artifact_outputs.clone(),
                    external_dependencies: unit.external_dependencies.clone(),
                    link: unit.link.clone(),
                });
            }
        }

        ActionPlan {
            build_nodes,
            compile_actions,
            link_actions,
        }
    }
}

pub(super) fn collect_build_nodes<'a>(
    build_nodes: &'a [StagedAction],
    ids: &[usize],
) -> Vec<&'a StagedAction> {
    ids.iter()
        .filter_map(|id| build_nodes.iter().find(|action| action.id == *id))
        .collect()
}
