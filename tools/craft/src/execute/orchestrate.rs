//! Public orchestration entry points for Craft execution commands.
//!
//! The thin wrappers select the command mode and delegate to the shared
//! execution engine so build, check, run, and test stay behaviorally aligned.

use super::*;

#[cfg_attr(not(test), allow(dead_code))]
pub fn build(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Build,
        None,
        false,
    )
}

pub fn build_with_progress_and_timings(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    progress: Option<ProgressReporter>,
    report_timings: bool,
) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Build,
        progress,
        report_timings,
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn check(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Check,
        None,
        false,
    )
}

pub fn check_with_progress_and_timings(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    progress: Option<ProgressReporter>,
    report_timings: bool,
) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Check,
        progress,
        report_timings,
    )
}

pub(crate) fn materialize_analysis_inputs(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
) -> Result<()> {
    materialize_analysis_inputs_with_progress(build_plan, action_plan, None)
}

pub(crate) fn materialize_analysis_inputs_with_progress(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    progress: Option<ProgressReporter>,
) -> Result<()> {
    let source_config = load_source_config(build_plan)?;
    let profile_selection = profile_selection_for_action_plan(action_plan);
    let mut built_std_packages = BTreeMap::new();
    let mut driver_families = BTreeMap::new();
    let mut summary = ExecutionSummary::default();
    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Bootstrap);
        progress.set_detail("prepare semantic inputs");
    }
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        crate::script::ScriptCommand::Build,
        &mut built_std_packages,
        &mut driver_families,
        &mut summary,
    )?;
    let mut built_external_packages = BTreeMap::new();
    let mut built_external_tools = BTreeMap::new();
    let mut external_build_stack = BTreeSet::new();
    let mut manifest_runtime_options = BTreeMap::new();
    let compile_action_index = compile_actions_index(&action_plan.compile_actions);
    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    let link_action_index = link_actions_by_artifact_path(&action_plan.link_actions);
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    let indexes = ActionIndexes {
        action_plan,
        compile_action_index: &compile_action_index,
        local_library_actions: &local_library_actions,
        link_action_index: &link_action_index,
    };
    let config = ExecutionConfig {
        source_config: &source_config,
        dependency_workspace_root: &build_plan.workspace_root,
        command: crate::script::ScriptCommand::Build,
        profile_selection,
        std_workspace_root: &build_plan.workspace_root,
        report_timings: false,
    };
    let mut session = ExecutionSession {
        indexes,
        config,
        external: ExternalArtifacts {
            built_std_packages: &mut built_std_packages,
            built_external_packages: &mut built_external_packages,
            built_external_tools: &mut built_external_tools,
            external_build_stack: &mut external_build_stack,
            manifest_runtime_options: &mut manifest_runtime_options,
            driver_families: &mut driver_families,
        },
        state: ExecutionState {
            compiled: &mut compiled,
            linked: &mut linked,
            staged_outputs: &mut staged_outputs,
            execution_summary: &mut summary,
            progress: progress.clone(),
        },
    };

    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Stage);
        progress.set_detail("materialize inputs");
    }
    for action in &action_plan.compile_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        cleanup_stale_compile_inputs(action, action_plan.build_nodes.as_slice())?;
        execute_staged_actions(
            action.compile_inputs.as_slice(),
            action_plan.build_nodes.as_slice(),
            action.required_source_path(),
            &mut session,
        )?;
    }

    Ok(())
}

pub(super) fn build_with_command(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    command: crate::script::ScriptCommand,
    progress: Option<ProgressReporter>,
    report_timings: bool,
) -> Result<ExecutionSummary> {
    let source_config = load_source_config(build_plan)?;
    let profile_selection = profile_selection_for_action_plan(action_plan);
    let mut built_std_packages = BTreeMap::new();
    let mut driver_families = BTreeMap::new();
    let mut external_summary = ExecutionSummary::default();
    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Bootstrap);
        progress.set_detail("prepare runtime packages");
    }
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        command,
        &mut built_std_packages,
        &mut driver_families,
        &mut external_summary,
    )?;
    let mut built_external_packages = BTreeMap::new();
    let mut built_external_tools = BTreeMap::new();
    let mut external_build_stack = BTreeSet::new();
    let mut manifest_runtime_options = BTreeMap::new();
    let config = ExecutionConfig {
        source_config: &source_config,
        dependency_workspace_root: &build_plan.workspace_root,
        command,
        profile_selection,
        std_workspace_root: &build_plan.workspace_root,
        report_timings,
    };
    {
        let mut external = ExternalArtifacts {
            built_std_packages: &mut built_std_packages,
            built_external_packages: &mut built_external_packages,
            built_external_tools: &mut built_external_tools,
            external_build_stack: &mut external_build_stack,
            manifest_runtime_options: &mut manifest_runtime_options,
            driver_families: &mut driver_families,
        };
        for dep in requested_external_dependencies(action_plan) {
            build_external_package(&dep, config, &mut external, &mut external_summary)?;
        }
    }

    let compile_action_index = compile_actions_index(&action_plan.compile_actions);
    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    let link_action_index = link_actions_by_artifact_path(&action_plan.link_actions);
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    let mut local_summary = ExecutionSummary::default();
    let indexes = ActionIndexes {
        action_plan,
        compile_action_index: &compile_action_index,
        local_library_actions: &local_library_actions,
        link_action_index: &link_action_index,
    };
    {
        let mut session = ExecutionSession {
            indexes,
            config,
            external: ExternalArtifacts {
                built_std_packages: &mut built_std_packages,
                built_external_packages: &mut built_external_packages,
                built_external_tools: &mut built_external_tools,
                external_build_stack: &mut external_build_stack,
                manifest_runtime_options: &mut manifest_runtime_options,
                driver_families: &mut driver_families,
            },
            state: ExecutionState {
                compiled: &mut compiled,
                linked: &mut linked,
                staged_outputs: &mut staged_outputs,
                execution_summary: &mut local_summary,
                progress: progress.clone(),
            },
        };

        if let Some(progress) = &progress {
            progress.set_phase(ExecutionPhase::Stage);
            progress.set_detail("materialize inputs");
        }
        for action in &action_plan.compile_actions {
            if action.domain != BuildDomain::Target {
                continue;
            }
            cleanup_stale_compile_inputs(action, action_plan.build_nodes.as_slice())?;
            execute_staged_actions(
                action.compile_inputs.as_slice(),
                action_plan.build_nodes.as_slice(),
                action.required_source_path(),
                &mut session,
            )?;
        }
    }

    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Compile);
        progress.set_detail("compile targets");
    }
    loop {
        let jobs = parallel_target_compile_jobs(action_plan, &local_library_actions, &compiled);
        if jobs.len() < 2 {
            break;
        }
        if let Some(progress) = &progress {
            progress.set_detail(format!("compile parallel batch ({} jobs)", jobs.len()));
        }
        let _progress_suspend = progress
            .as_ref()
            .map(|progress| progress.suspend_terminal());
        let _long_action = progress.as_ref().map(|progress| {
            progress.report_long_action(
                "compiling",
                format!("parallel target batch ({} jobs)", jobs.len()),
            )
        });
        for result in build_parallel_target_compile_jobs(
            command,
            &jobs,
            &local_library_actions,
            &built_std_packages,
            &built_external_packages,
            config.report_timings,
        )? {
            compiled.insert(result.compile_object_path);
            local_summary.absorb(result.summary);
            if let Some(progress) = &progress {
                progress.record_compile_action();
            }
        }
    }

    {
        let mut session = ExecutionSession {
            indexes,
            config,
            external: ExternalArtifacts {
                built_std_packages: &mut built_std_packages,
                built_external_packages: &mut built_external_packages,
                built_external_tools: &mut built_external_tools,
                external_build_stack: &mut external_build_stack,
                manifest_runtime_options: &mut manifest_runtime_options,
                driver_families: &mut driver_families,
            },
            state: ExecutionState {
                compiled: &mut compiled,
                linked: &mut linked,
                staged_outputs: &mut staged_outputs,
                execution_summary: &mut local_summary,
                progress: progress.clone(),
            },
        };

        for action in &action_plan.compile_actions {
            if action.domain != BuildDomain::Target
                || action.target_kind != crate::plan::TargetKind::Lib
            {
                continue;
            }
            ensure_compile_action_built(action, &mut session)?;
        }

        if command == crate::script::ScriptCommand::Check {
            for action in &action_plan.compile_actions {
                if action.domain != BuildDomain::Target {
                    continue;
                }
                ensure_compile_action_built(action, &mut session)?;
            }
        }
    }

    if command == crate::script::ScriptCommand::Check {
        if let Some(progress) = &progress {
            progress.set_phase(ExecutionPhase::Finalize);
            progress.set_detail("summarize results");
        }
        external_summary.absorb(local_summary);
        if let Some(progress) = &progress {
            progress.record_finalize_action();
        }
        return Ok(external_summary);
    }

    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Link);
        progress.set_detail("link targets");
    }
    let parallel_jobs = parallel_target_link_jobs(action_plan, &compile_action_index, &linked)?;
    if let Some(progress) = &progress
        && !parallel_jobs.is_empty()
    {
        progress.set_detail(format!(
            "link parallel batch ({} jobs)",
            parallel_jobs.len()
        ));
    }
    let _progress_suspend = progress
        .as_ref()
        .map(|progress| progress.suspend_terminal());
    let _long_action = progress.as_ref().and_then(|progress| {
        (!parallel_jobs.is_empty()).then(|| {
            progress.report_long_action(
                "linking",
                format!("parallel target batch ({} jobs)", parallel_jobs.len()),
            )
        })
    });
    for result in build_parallel_target_link_jobs(
        command,
        &parallel_jobs,
        &local_library_actions,
        &built_std_packages,
        &built_external_packages,
        config.report_timings,
    )? {
        compiled.insert(result.compile_object_path);
        linked.insert(result.artifact_path);
        local_summary.absorb(result.summary);
        if let Some(progress) = &progress {
            progress.record_compile_action();
            progress.record_link_action();
        }
    }

    {
        let mut session = ExecutionSession {
            indexes,
            config,
            external: ExternalArtifacts {
                built_std_packages: &mut built_std_packages,
                built_external_packages: &mut built_external_packages,
                built_external_tools: &mut built_external_tools,
                external_build_stack: &mut external_build_stack,
                manifest_runtime_options: &mut manifest_runtime_options,
                driver_families: &mut driver_families,
            },
            state: ExecutionState {
                compiled: &mut compiled,
                linked: &mut linked,
                staged_outputs: &mut staged_outputs,
                execution_summary: &mut local_summary,
                progress: progress.clone(),
            },
        };

        for action in &action_plan.link_actions {
            if action.domain != BuildDomain::Target {
                continue;
            }
            ensure_link_action_built(action, &mut session)?;
        }
    }

    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Finalize);
        progress.set_detail("summarize results");
    }
    external_summary.absorb(local_summary);
    if let Some(progress) = &progress {
        progress.record_finalize_action();
    }
    Ok(external_summary)
}
