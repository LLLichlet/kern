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
use std::collections::{BTreeSet, HashMap};
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

    fn ensure_executable_artifact_phase(&self, operation: &str) -> std::result::Result<(), String> {
        if self.unit.artifact_kind == crate::build_plan::ArtifactKind::Executable {
            return Ok(());
        }

        Err(format!(
            "`{operation}` is only supported for executable units; current unit kind is `{:?}`",
            self.unit.target_kind
        ))
    }

    fn expect_bound_output(
        &self,
        args: &[ConstValue],
        index: usize,
        label: &str,
    ) -> std::result::Result<BuildOutput, String> {
        let output = expect_output(args, index, label)?;
        self.validate_output_handle(&output, label)?;
        Ok(output)
    }

    fn validate_output_handle(
        &self,
        output: &BuildOutput,
        label: &str,
    ) -> std::result::Result<(), String> {
        match output.kind {
            BuildOutputKind::Staged { id, phase } => {
                let unit_node_ids = unit_bound_node_ids(self.unit, phase);
                if !unit_node_ids.contains(&id) {
                    return Err(format!(
                        "`{label}` must refer to a staged build output declared by the current unit"
                    ));
                }
                let Some(action) = self.build_nodes.iter().find(|action| action.id == id) else {
                    return Err(format!(
                        "`{label}` refers to unknown staged build output id `{id}`"
                    ));
                };
                let action_path = {
                    let output_path = Path::new(&action.output);
                    if output_path.is_absolute() {
                        normalized_path_string(output_path)
                    } else {
                        normalized_path_string(
                            &self.script_context.workspace_root_path.join(output_path),
                        )
                    }
                };
                if action.phase != phase || action_path != output.path {
                    return Err(format!(
                        "`{label}` must refer to a staged build output declared by the current unit"
                    ));
                }
                Ok(())
            }
            BuildOutputKind::PrimaryArtifact => {
                self.ensure_executable_artifact_phase("primary_artifact()")?;
                if output.path != self.script_context.paths.artifact_path {
                    return Err(format!(
                        "`{label}` must refer to the current unit primary artifact"
                    ));
                }
                Ok(())
            }
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
            "__craft_build_primary_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("primary_artifact()")?;
                Ok(output_value(&BuildOutput {
                    kind: BuildOutputKind::PrimaryArtifact,
                    path: self.script_context.paths.artifact_path.clone(),
                }))
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
                let output = self.expect_bound_output(args, 1, "output")?;
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
                let resolved = normalized_path_string(&resolved_path);
                self.unit.link.args.push(resolved.clone());
                push_unique(&mut self.unit.link.input_paths, resolved);
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
                let output = self.expect_bound_output(args, 1, "output")?;
                let BuildOutputKind::Staged {
                    id,
                    phase: StagedActionPhase::PreCompile,
                } = output.kind
                else {
                    return Err(
                        "source root can only be bound from pre-compile staged outputs".to_string(),
                    );
                };
                self.unit.source_root = SourceRootBinding::BuildOutput {
                    id,
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
                )?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_output" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_output = self.expect_bound_output(args, 1, "source output")?;
                let BuildOutputKind::Staged {
                    id: dependency_id,
                    phase: StagedActionPhase::PreCompile,
                } = source_output.kind
                else {
                    return Err(
                        "generated outputs can only copy from pre-compile staged outputs"
                            .to_string(),
                    );
                };
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
                )?;
                add_staged_dependency(
                    self.build_nodes,
                    output.staged_id("generated output")?,
                    dependency_id,
                )?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_artifact_file" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("stage_artifact_file(...)")?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_artifact_file_from_tool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("stage_artifact_file_from_tool(...)")?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_output_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("stage_copy_output_to_artifact(...)")?;
                let source_output = self.expect_bound_output(args, 1, "source output")?;
                let dependency_id = source_output.dependency_id_for_post_link_copy();
                let artifact_relative = expect_string(args, 2, "artifact relative path")?;
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
                    StagedActionKind::CopyFile {
                        source: source_output.path,
                    },
                )?;
                if let Some(dependency_id) = dependency_id {
                    add_staged_dependency(
                        self.build_nodes,
                        output.staged_id("artifact output")?,
                        dependency_id,
                    )?;
                }
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_file_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("stage_copy_package_file_to_artifact(...)")?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_dir_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("stage_copy_package_dir_to_artifact(...)")?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_resource_file_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("stage_copy_resource_file_to_artifact(...)")?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_resource_dir_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                self.ensure_executable_artifact_phase("stage_copy_resource_dir_to_artifact(...)")?;
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
                )?;
                Ok(output_value(&output))
            }
            "__craft_build_depend" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let output = self.expect_bound_output(args, 1, "output")?;
                let dependency = self.expect_bound_output(args, 2, "dependency")?;
                add_staged_dependency(
                    self.build_nodes,
                    output.staged_id("output")?,
                    dependency.staged_id("dependency")?,
                )?;
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
    kind: BuildOutputKind,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildOutputKind {
    Staged { id: usize, phase: StagedActionPhase },
    PrimaryArtifact,
}

impl BuildOutput {
    fn staged_id(&self, label: &str) -> std::result::Result<usize, String> {
        match self.kind {
            BuildOutputKind::Staged { id, .. } => Ok(id),
            BuildOutputKind::PrimaryArtifact => {
                Err(format!("`{label}` must refer to a staged build output"))
            }
        }
    }

    fn dependency_id_for_post_link_copy(&self) -> Option<usize> {
        match self.kind {
            BuildOutputKind::Staged {
                id,
                phase: StagedActionPhase::PostLink,
            } => Some(id),
            BuildOutputKind::Staged {
                phase: StagedActionPhase::PreCompile,
                ..
            }
            | BuildOutputKind::PrimaryArtifact => None,
        }
    }
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
) -> std::result::Result<BuildOutput, String> {
    let output = relative_display(workspace_root, path);
    let output_path = PathBuf::from(&output);
    let node_ids = unit_bound_node_ids(unit, phase);
    for id in node_ids {
        let existing = build_nodes
            .iter()
            .find(|action| action.id == *id)
            .expect("build node id must exist");
        let existing_path = Path::new(&existing.output);
        if existing_path == output_path {
            return Err(format!(
                "{} output `{}` is already declared",
                staged_phase_label(phase),
                output_path.display()
            ));
        }
        if existing_path.starts_with(&output_path) || output_path.starts_with(existing_path) {
            return Err(format!(
                "{} output `{}` conflicts with existing output `{}`; staged outputs within a single phase must not overlap",
                staged_phase_label(phase),
                output_path.display(),
                existing_path.display()
            ));
        }
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
    Ok(BuildOutput {
        kind: BuildOutputKind::Staged { id, phase },
        path: normalized_path_string(path),
    })
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
    if staged_dependency_reaches(build_nodes, dependency_id, output_id) {
        return Err("build output dependencies must not contain cycles".to_string());
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

fn staged_dependency_reaches(
    build_nodes: &[StagedAction],
    start_id: usize,
    target_id: usize,
) -> bool {
    let mut stack = vec![start_id];
    let mut visited = BTreeSet::new();

    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        if id == target_id {
            return true;
        }

        let Some(action) = build_nodes.iter().find(|action| action.id == id) else {
            continue;
        };
        stack.extend(action.depends_on.iter().copied());
    }

    false
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
    match output.kind {
        BuildOutputKind::Staged {
            id,
            phase: StagedActionPhase::PreCompile,
        } => ConstValue::String(format!("pre|{}|{}", id, output.path)),
        BuildOutputKind::Staged {
            id,
            phase: StagedActionPhase::PostLink,
        } => ConstValue::String(format!("post|{}|{}", id, output.path)),
        BuildOutputKind::PrimaryArtifact => ConstValue::String(format!("artifact|{}", output.path)),
    }
}

fn staged_phase_label(phase: StagedActionPhase) -> &'static str {
    match phase {
        StagedActionPhase::PreCompile => "pre-compile",
        StagedActionPhase::PostLink => "post-link",
    }
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
    let mut parts = value.splitn(3, '|');
    let kind = parts
        .next()
        .ok_or_else(|| format!("expected `{label}` to be a build output handle"))?;

    match kind {
        "pre" | "post" => {
            let id = parts
                .next()
                .ok_or_else(|| format!("expected `{label}` to carry a build output id"))?
                .parse::<usize>()
                .map_err(|_| format!("expected `{label}` to carry a numeric build output id"))?;
            let path = parts
                .next()
                .ok_or_else(|| format!("expected `{label}` to carry a build output path"))?;
            if path.is_empty() {
                return Err(format!("expected `{label}` to carry a build output path"));
            }
            Ok(BuildOutput {
                kind: BuildOutputKind::Staged {
                    id,
                    phase: if kind == "pre" {
                        StagedActionPhase::PreCompile
                    } else {
                        StagedActionPhase::PostLink
                    },
                },
                path: path.to_string(),
            })
        }
        "artifact" => {
            let path = parts
                .next()
                .ok_or_else(|| format!("expected `{label}` to carry a build output path"))?;
            if path.is_empty() {
                return Err(format!("expected `{label}` to carry a build output path"));
            }
            Ok(BuildOutput {
                kind: BuildOutputKind::PrimaryArtifact,
                path: path.to_string(),
            })
        }
        _ => Err(format!("expected `{label}` to be a build output handle")),
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}
