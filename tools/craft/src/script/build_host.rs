use super::{
    BuildScriptContext, BuildScriptTool, build_domain_tag, expect_arg, expect_bool, expect_string,
    option_none, option_some, plan_argument_value, pure_enum_value, target_kind_tag,
};
use crate::build_plan::{
    BuildUnit, GeneratedFile, GeneratedFileOrigin, SourceRootBinding, StagedAction,
    StagedActionKind, StagedActionPhase,
};
use crate::plan::PlanValue;
use kernc_sema::checker::{ConstValue, ScriptHost};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub(super) struct BuildUnitHost<'a> {
    build_nodes: &'a mut Vec<StagedAction>,
    unit: &'a mut BuildUnit,
    script_context: &'a BuildScriptContext,
}

impl<'a> BuildUnitHost<'a> {
    pub(super) fn new(
        build_nodes: &'a mut Vec<StagedAction>,
        unit: &'a mut BuildUnit,
        script_context: &'a BuildScriptContext,
    ) -> Self {
        Self {
            build_nodes,
            unit,
            script_context,
        }
    }
}

impl ScriptHost for BuildUnitHost<'_> {
    fn call_extern(
        &mut self,
        name: &str,
        args: &[ConstValue],
        _span: Span,
    ) -> std::result::Result<ConstValue, String> {
        match name {
            "__craft_build_feature_enabled" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let feature = expect_string(args, 1, "feature name")?;
                Ok(ConstValue::Bool(
                    self.script_context
                        .script
                        .features
                        .contains(feature.as_str()),
                ))
            }
            "__craft_build_tool_path" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let dependency_name = expect_string(args, 1, "tool dependency name")?;
                let tool_name = expect_string(args, 2, "tool target name")?;
                let tool = resolve_build_tool(self.script_context, &dependency_name, &tool_name)?;
                Ok(ConstValue::String(tool.executable_path.clone()))
            }
            "__craft_build_resource_root" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let resource_name = expect_string(args, 1, "resource name")?;
                let resource = resolve_build_resource(self.script_context, &resource_name)?;
                Ok(ConstValue::String(resource.root_path.clone()))
            }
            "__craft_build_resource_path" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let resource_name = expect_string(args, 1, "resource name")?;
                let relative_path = expect_string(args, 2, "resource relative path")?;
                let resource = resolve_build_resource(self.script_context, &resource_name)?;
                let resolved_path =
                    resource_input_path(Path::new(&resource.root_path), &relative_path)?;
                if !resolved_path.exists() {
                    return Err(format!(
                        "resource path `{}` does not exist",
                        resolved_path.display()
                    ));
                }
                Ok(ConstValue::String(normalized_path_string(&resolved_path)))
            }
            "__craft_build_output_path" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let output = expect_output(args, 1, "output")?;
                Ok(ConstValue::String(output.path))
            }
            "__craft_build_link_system_lib" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let name = expect_string(args, 1, "system library name")?;
                push_unique(&mut self.unit.link.system_libs, name);
                Ok(ConstValue::Void)
            }
            "__craft_build_link_framework" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let name = expect_string(args, 1, "framework name")?;
                push_unique(&mut self.unit.link.frameworks, name);
                Ok(ConstValue::Void)
            }
            "__craft_build_link_search" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let path = expect_string(args, 1, "link search path")?;
                push_unique(&mut self.unit.link.search_paths, path);
                Ok(ConstValue::Void)
            }
            "__craft_build_link_arg" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let arg = expect_string(args, 1, "link argument")?;
                self.unit.link.args.push(arg);
                Ok(ConstValue::Void)
            }
            "__craft_build_link_arg_path" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let flag = expect_string(args, 1, "link argument flag")?;
                if flag.trim().is_empty() {
                    return Err("link argument flag must not be empty".to_string());
                }
                let path = expect_string(args, 2, "link argument path")?;
                let resolved_path = package_or_absolute_path(
                    &self.script_context.package_root_path,
                    &path,
                    "link argument path",
                )?;
                if !resolved_path.exists() {
                    return Err(format!(
                        "link argument path `{}` does not exist",
                        resolved_path.display()
                    ));
                }
                self.unit.link.args.push(flag);
                self.unit
                    .link
                    .args
                    .push(normalized_path_string(&resolved_path));
                Ok(ConstValue::Void)
            }
            "__craft_build_cfg_bool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_bool(args, 2, "cfg value")?;
                self.unit.cfg.insert(key, PlanValue::Bool(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_cfg_string" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_string(args, 2, "cfg value")?;
                self.unit.cfg.insert(key, PlanValue::String(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_define_bool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_bool(args, 2, "define value")?;
                self.unit.define.insert(key, PlanValue::Bool(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_define_string" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_string(args, 2, "define value")?;
                self.unit.define.insert(key, PlanValue::String(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_set_source_root" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let path = expect_string(args, 1, "source root")?;
                self.unit.source_root = source_root_binding_from_script_path(&path)?;
                Ok(ConstValue::Void)
            }
            "__craft_build_set_source_root_from" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let output = expect_output(args, 1, "output")?;
                self.unit.source_root = SourceRootBinding::BuildOutput {
                    id: output.id,
                    path: normalize_path_display(&output.path),
                };
                Ok(ConstValue::Void)
            }
            "__craft_build_stage_generated" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let relative_path = expect_string(args, 1, "generated relative path")?;
                let contents = expect_string(args, 2, "generated file contents")?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &relative_path,
                )?;
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Emitted,
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::WriteFile { contents },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_file" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_relative = expect_string(args, 1, "package relative source path")?;
                let generated_relative = expect_string(args, 2, "generated relative path")?;
                let source_path =
                    package_input_path(&self.script_context.package_root_path, &source_relative)?;
                if !source_path.is_file() {
                    return Err(format!(
                        "package source file `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &generated_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Copied {
                        source: source.clone(),
                    },
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::CopyFile { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_output" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_output = expect_output(args, 1, "source output")?;
                let generated_relative = expect_string(args, 2, "generated relative path")?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &generated_relative,
                )?;
                let source_display = relative_display(
                    &self.script_context.workspace_root_path,
                    Path::new(&source_output.path),
                );
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Copied {
                        source: source_display,
                    },
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::CopyFile {
                        source: source_output.path,
                    },
                );
                add_staged_dependency(self.build_nodes, output.id, source_output.id)?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_generated_from_tool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let dependency_name = expect_string(args, 1, "tool dependency name")?;
                let tool_name = expect_string(args, 2, "tool target name")?;
                let generated_relative = expect_string(args, 3, "generated relative path")?;
                let args = expect_string_list(args, 4, "tool arguments")?;
                let tool = resolve_build_tool(self.script_context, &dependency_name, &tool_name)?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &generated_relative,
                )?;
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Emitted,
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::RunTool {
                        tool: Box::new(tool.clone()),
                        args,
                    },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_artifact_file" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let relative_path = expect_string(args, 1, "artifact relative path")?;
                let contents = expect_string(args, 2, "artifact file contents")?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &relative_path,
                )?;
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::WriteFile { contents },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_artifact_file_from_tool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let dependency_name = expect_string(args, 1, "tool dependency name")?;
                let tool_name = expect_string(args, 2, "tool target name")?;
                let artifact_relative = expect_string(args, 3, "artifact relative path")?;
                let args = expect_string_list(args, 4, "tool arguments")?;
                let tool = resolve_build_tool(self.script_context, &dependency_name, &tool_name)?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::RunTool {
                        tool: Box::new(tool.clone()),
                        args,
                    },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_file_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_relative = expect_string(args, 1, "package relative source path")?;
                let artifact_relative = expect_string(args, 2, "artifact relative path")?;
                let source_path =
                    package_input_path(&self.script_context.package_root_path, &source_relative)?;
                if !source_path.is_file() {
                    return Err(format!(
                        "package source file `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::CopyFile { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_dir_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_relative = expect_string(args, 1, "package relative source dir")?;
                let artifact_relative = expect_string(args, 2, "artifact relative dir")?;
                let source_path =
                    package_input_path(&self.script_context.package_root_path, &source_relative)?;
                if !source_path.is_dir() {
                    return Err(format!(
                        "package source directory `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::CopyDirectory { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_resource_file_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let resource_name = expect_string(args, 1, "resource name")?;
                let source_relative = expect_string(args, 2, "resource relative source path")?;
                let artifact_relative = expect_string(args, 3, "artifact relative path")?;
                let resource = resolve_build_resource(self.script_context, &resource_name)?;
                let source_path =
                    resource_input_path(Path::new(&resource.root_path), &source_relative)?;
                if !source_path.is_file() {
                    return Err(format!(
                        "resource source file `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::CopyFile { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_resource_dir_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let resource_name = expect_string(args, 1, "resource name")?;
                let source_relative = expect_string(args, 2, "resource relative source dir")?;
                let artifact_relative = expect_string(args, 3, "artifact relative dir")?;
                let resource = resolve_build_resource(self.script_context, &resource_name)?;
                let source_path =
                    resource_input_path(Path::new(&resource.root_path), &source_relative)?;
                if !source_path.is_dir() {
                    return Err(format!(
                        "resource source directory `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::CopyDirectory { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_depend" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let output = expect_output(args, 1, "output")?;
                let dependency = expect_output(args, 2, "dependency")?;
                add_staged_dependency(self.build_nodes, output.id, dependency.id)?;
                Ok(ConstValue::Void)
            }
            _ => Err(format!("unsupported build host function `{name}`")),
        }
    }
}

pub(super) fn build_argument_value(
    ctx: &mut kernc_sema::SemaContext<'_>,
    script_context: &BuildScriptContext,
) -> ConstValue {
    fn field(name: &str, ctx: &mut kernc_sema::SemaContext<'_>) -> SymbolId {
        ctx.intern(name)
    }

    let mut unit = HashMap::new();
    unit.insert(
        field("domain", ctx),
        pure_enum_value(build_domain_tag(script_context.unit.domain)),
    );
    unit.insert(
        field("kind", ctx),
        pure_enum_value(target_kind_tag(script_context.unit.target_kind)),
    );
    unit.insert(
        field("name", ctx),
        match &script_context.unit.target_name {
            Some(name) => option_some(ConstValue::String(name.clone())),
            None => option_none(),
        },
    );
    unit.insert(
        field("source_root", ctx),
        ConstValue::String(script_context.unit.source_root.clone()),
    );
    unit.insert(
        field("artifact_name", ctx),
        ConstValue::String(script_context.unit.artifact_name.clone()),
    );

    let mut builder = match plan_argument_value(ctx, &script_context.script) {
        ConstValue::Struct(value) => value,
        _ => unreachable!("plan_argument_value must return a struct"),
    };
    builder.insert(field("unit", ctx), ConstValue::Struct(unit));
    let mut paths = HashMap::new();
    paths.insert(
        field("build_root", ctx),
        ConstValue::String(script_context.paths.build_root.clone()),
    );
    paths.insert(
        field("generated_root", ctx),
        ConstValue::String(script_context.paths.generated_root.clone()),
    );
    paths.insert(
        field("artifact_root", ctx),
        ConstValue::String(script_context.paths.artifact_root.clone()),
    );
    paths.insert(
        field("object", ctx),
        ConstValue::String(script_context.paths.object_path.clone()),
    );
    paths.insert(
        field("artifact", ctx),
        ConstValue::String(script_context.paths.artifact_path.clone()),
    );
    paths.insert(
        field("metadata", ctx),
        match &script_context.paths.metadata_path {
            Some(path) => option_some(ConstValue::String(path.clone())),
            None => option_none(),
        },
    );
    builder.insert(field("paths", ctx), ConstValue::Struct(paths));

    ConstValue::Struct(builder)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildOutput {
    id: usize,
    path: String,
}

fn generated_output_path(root: &Path, relative_path: &str) -> std::result::Result<PathBuf, String> {
    Ok(root.join(normalize_relative_path(
        relative_path,
        "generated relative path",
    )?))
}

fn source_root_binding_from_script_path(
    path: &str,
) -> std::result::Result<SourceRootBinding, String> {
    if path.trim().is_empty() {
        return Err("source root must not be empty".to_string());
    }
    if Path::new(path).is_absolute() {
        return Ok(SourceRootBinding::AbsolutePath(normalize_path_display(
            path,
        )));
    }
    Ok(SourceRootBinding::PackagePath(normalize_relative_display(
        path,
        "source root",
    )?))
}

fn package_input_path(root: &Path, relative_path: &str) -> std::result::Result<PathBuf, String> {
    Ok(root.join(normalize_relative_path(
        relative_path,
        "package relative source path",
    )?))
}

fn resource_input_path(root: &Path, relative_path: &str) -> std::result::Result<PathBuf, String> {
    Ok(root.join(normalize_relative_path(
        relative_path,
        "resource relative source path",
    )?))
}

fn package_or_absolute_path(
    root: &Path,
    path: &str,
    label: &str,
) -> std::result::Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Ok(candidate.to_path_buf());
    }

    Ok(root.join(normalize_relative_path(path, label)?))
}

fn normalize_relative_path(
    relative_path: &str,
    label: &str,
) -> std::result::Result<PathBuf, String> {
    if relative_path.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }

    let path = Path::new(relative_path);
    if path.is_absolute() {
        return Err(format!("{label} must not be absolute"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("{label} must not contain `..`"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{label} must stay within its declared root"));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(format!("{label} must not be empty"));
    }

    Ok(normalized)
}

fn normalize_relative_display(
    relative_path: &str,
    label: &str,
) -> std::result::Result<String, String> {
    Ok(normalize_relative_path(relative_path, label)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn normalize_path_display(path: &str) -> String {
    path.replace('\\', "/")
}

fn normalized_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn record_generated_file(
    unit: &mut BuildUnit,
    workspace_root: &Path,
    path: &Path,
    origin: GeneratedFileOrigin,
) {
    let path = relative_display(workspace_root, path);
    if let Some(existing) = unit
        .generated_files
        .iter_mut()
        .find(|entry| entry.path == path)
    {
        existing.origin = origin;
        return;
    }
    unit.generated_files.push(GeneratedFile { path, origin });
}

fn record_staged_action(
    build_nodes: &mut Vec<StagedAction>,
    unit: &mut BuildUnit,
    workspace_root: &Path,
    path: &Path,
    phase: StagedActionPhase,
    kind: StagedActionKind,
) -> BuildOutput {
    let output = relative_display(workspace_root, path);
    let node_ids = unit_bound_node_ids(unit, phase);
    if let Some(existing_id) = node_ids.iter().copied().find(|id| {
        build_nodes
            .iter()
            .any(|action| action.id == *id && action.phase == phase && action.output == output)
    }) {
        let existing = build_nodes
            .iter_mut()
            .find(|action| action.id == existing_id)
            .expect("build node id must exist");
        existing.kind = kind;
        return BuildOutput {
            id: existing_id,
            path: normalized_path_string(path),
        };
    }
    let id = next_staged_action_id(build_nodes);
    build_nodes.push(StagedAction {
        id,
        phase,
        output,
        depends_on: Vec::new(),
        kind,
    });
    unit_bound_node_ids_mut(unit, phase).push(id);
    BuildOutput {
        id,
        path: normalized_path_string(path),
    }
}

fn next_staged_action_id(build_nodes: &[StagedAction]) -> usize {
    build_nodes
        .iter()
        .map(|action| action.id)
        .max()
        .unwrap_or(0)
        + 1
}

fn add_staged_dependency(
    build_nodes: &mut [StagedAction],
    output_id: usize,
    dependency_id: usize,
) -> std::result::Result<(), String> {
    if output_id == dependency_id {
        return Err("build outputs cannot depend on themselves".to_string());
    }

    let output_phase = build_nodes
        .iter()
        .find(|action| action.id == output_id)
        .map(|action| action.phase)
        .ok_or_else(|| format!("unknown build output id `{output_id}`"))?;
    let dependency_phase = build_nodes
        .iter()
        .find(|action| action.id == dependency_id)
        .map(|action| action.phase)
        .ok_or_else(|| format!("unknown build output id `{dependency_id}`"))?;
    if output_phase != dependency_phase {
        return Err("build output dependencies must stay within a single stage phase".to_string());
    }

    let action = build_nodes
        .iter_mut()
        .find(|action| action.id == output_id)
        .expect("staged action must exist after phase lookup");
    if !action.depends_on.contains(&dependency_id) {
        action.depends_on.push(dependency_id);
    }
    Ok(())
}

fn unit_bound_node_ids(unit: &BuildUnit, phase: StagedActionPhase) -> &[usize] {
    match phase {
        StagedActionPhase::PreCompile => &unit.build.compile_inputs,
        StagedActionPhase::PostLink => &unit.build.artifact_outputs,
    }
}

fn unit_bound_node_ids_mut(unit: &mut BuildUnit, phase: StagedActionPhase) -> &mut Vec<usize> {
    match phase {
        StagedActionPhase::PreCompile => &mut unit.build.compile_inputs,
        StagedActionPhase::PostLink => &mut unit.build.artifact_outputs,
    }
}

fn output_value(output: &BuildOutput) -> ConstValue {
    ConstValue::String(format!("{}|{}", output.id, output.path))
}

fn resolve_build_tool<'a>(
    script_context: &'a BuildScriptContext,
    dependency_name: &str,
    tool_name: &str,
) -> std::result::Result<&'a BuildScriptTool, String> {
    let Some(tools) = script_context.tools.get(dependency_name) else {
        return Err(format!(
            "build dependency `{dependency_name}` does not expose a host tool"
        ));
    };
    tools.iter()
        .find(|tool| tool.target_name == tool_name)
        .ok_or_else(|| {
            format!(
                "build dependency `{dependency_name}` does not expose host tool `{tool_name}` (available: {})",
                tools.iter()
                    .map(|tool| tool.target_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn resolve_build_resource<'a>(
    script_context: &'a BuildScriptContext,
    resource_name: &str,
) -> std::result::Result<&'a crate::script::BuildScriptResource, String> {
    script_context.resources.get(resource_name).ok_or_else(|| {
        format!("resource `{resource_name}` is not declared in `[resources]` for this package")
    })
}

fn expect_string_list(
    args: &[ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<Vec<String>, String> {
    match expect_arg(args, index, label)? {
        ConstValue::Array(values) => values
            .iter()
            .map(|value| match value {
                ConstValue::String(value) => Ok(value.clone()),
                _ => Err(format!("expected every `{label}` entry to be a string")),
            })
            .collect(),
        _ => Err(format!("expected `{label}` to be an array of strings")),
    }
}

fn expect_output(
    args: &[ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<BuildOutput, String> {
    let value = expect_string(args, index, label)?;
    let Some((id, path)) = value.split_once('|') else {
        return Err(format!("expected `{label}` to be a build output handle"));
    };
    let id = id
        .parse::<usize>()
        .map_err(|_| format!("expected `{label}` to carry a numeric build output id"))?;
    if path.is_empty() {
        return Err(format!("expected `{label}` to carry a build output path"));
    }
    Ok(BuildOutput {
        id,
        path: path.to_string(),
    })
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}
