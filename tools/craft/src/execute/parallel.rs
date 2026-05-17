use super::options::{compile_action_options, link_action_options};
use super::{
    BuiltExternalPackage, BuiltStdPackage, ExecutionSummary, ManifestRuntimeOptions,
    PackageInstanceKey, Result, build_compile_action_if_needed, build_link_action_if_needed,
    ensure_parent_dir,
};
use crate::build_plan::{CompileAction, LinkAction};
use crate::error::Error;
use crate::graph::BuildDomain;
use crate::resolver::ExternalPackageId;
use kernc_driver::{CompilerDriver, IncrementalDriverKey};
use kernc_utils::config::LtoMode;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

type DriverFamilyMap = BTreeMap<IncrementalDriverKey, CompilerDriver>;

pub(super) struct ParallelTargetLinkJob<'a> {
    pub(super) compile_action: &'a CompileAction,
    pub(super) link_action: &'a LinkAction,
}

pub(super) struct ParallelTargetCompileJob<'a> {
    pub(super) compile_action: &'a CompileAction,
}

pub(super) struct ParallelTargetCompileResult {
    pub(super) compile_object_path: PathBuf,
    pub(super) summary: ExecutionSummary,
}

pub(super) struct ParallelTargetLinkResult {
    pub(super) compile_object_path: PathBuf,
    pub(super) artifact_path: PathBuf,
    pub(super) summary: ExecutionSummary,
}

pub(super) fn compile_action_for_link_action<'a>(
    link_action: &LinkAction,
    compile_action_index: &'a BTreeMap<super::ActionKey, CompileAction>,
) -> Result<&'a CompileAction> {
    compile_action_index
        .get(&super::ActionKey {
            domain: link_action.domain,
            package_id: link_action.package_id.clone(),
            target_kind: link_action.target_kind,
            target_name: link_action.target_name.clone(),
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing compile action for `{}` target `{}`",
                link_action.package_id.name, link_action.artifact_name
            ))
        })
}

pub(super) fn parallel_target_link_jobs<'a>(
    action_plan: &'a super::ActionPlan,
    compile_action_index: &'a BTreeMap<super::ActionKey, CompileAction>,
    linked: &BTreeSet<PathBuf>,
) -> Result<Vec<ParallelTargetLinkJob<'a>>> {
    let mut jobs = Vec::new();
    for action in &action_plan.link_actions {
        if action.domain != BuildDomain::Target
            || !action.artifact_outputs.is_empty()
            || linked.contains(&action.artifact_path)
        {
            continue;
        }
        let compile_action = compile_action_for_link_action(action, compile_action_index)?;
        if link_job_prefers_serial_execution(compile_action) {
            continue;
        }
        jobs.push(ParallelTargetLinkJob {
            compile_action,
            link_action: action,
        });
    }
    Ok(jobs)
}

fn link_job_prefers_serial_execution(compile_action: &CompileAction) -> bool {
    compile_action.domain == BuildDomain::Target && compile_action.profile.lto_mode == LtoMode::Thin
}

fn compile_action_local_dependencies_ready(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    compiled: &BTreeSet<PathBuf>,
) -> bool {
    action.local_dependencies.iter().all(|dep| {
        local_library_actions
            .get(&PackageInstanceKey {
                domain: dep.domain,
                package_id: dep.package_id.clone(),
            })
            .is_none_or(|dep_action| compiled.contains(&dep_action.object_path))
    })
}

pub(super) fn parallel_target_compile_jobs<'a>(
    action_plan: &'a super::ActionPlan,
    local_library_actions: &'a BTreeMap<PackageInstanceKey, CompileAction>,
    compiled: &BTreeSet<PathBuf>,
) -> Vec<ParallelTargetCompileJob<'a>> {
    let mut jobs = Vec::new();
    for action in &action_plan.compile_actions {
        if action.domain != BuildDomain::Target
            || action.target_kind != crate::plan::TargetKind::Lib
            || compiled.contains(&action.object_path)
            || !compile_action_local_dependencies_ready(action, local_library_actions, compiled)
        {
            continue;
        }
        jobs.push(ParallelTargetCompileJob {
            compile_action: action,
        });
    }
    jobs
}

fn target_parallel_worker_count(job_count: usize) -> usize {
    if job_count < 2 {
        return 1;
    }

    #[cfg(test)]
    {
        return crate::test_support::test_parallel_worker_count(job_count);
    }

    #[cfg(not(test))]
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(job_count)
}

fn build_parallel_target_link_job(
    command: crate::script::ScriptCommand,
    job: &ParallelTargetLinkJob<'_>,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    manifest_runtime_options: &mut BTreeMap<PathBuf, ManifestRuntimeOptions>,
    driver_families: &mut DriverFamilyMap,
) -> Result<ParallelTargetLinkResult> {
    ensure_parent_dir(&job.compile_action.object_path)?;
    ensure_parent_dir(&job.compile_action.artifact_path)?;
    let compile_options = compile_action_options(
        command,
        job.compile_action,
        local_library_actions,
        built_std_packages,
        built_external_packages,
        manifest_runtime_options,
    )?;
    let mut summary = ExecutionSummary::default();
    let _ = build_compile_action_if_needed(
        job.compile_action,
        compile_options,
        driver_families,
        &mut summary,
        None,
    )?;

    ensure_parent_dir(&job.link_action.artifact_path)?;
    let (link_options, linker_inputs) = link_action_options(
        job.link_action,
        job.compile_action,
        local_library_actions,
        built_std_packages,
        built_external_packages,
        manifest_runtime_options,
    )?;
    let _ = build_link_action_if_needed(
        job.link_action,
        link_options,
        &linker_inputs,
        &mut summary,
        None,
    )?;

    Ok(ParallelTargetLinkResult {
        compile_object_path: job.compile_action.object_path.clone(),
        artifact_path: job.link_action.artifact_path.clone(),
        summary,
    })
}

fn build_parallel_target_compile_job(
    command: crate::script::ScriptCommand,
    job: &ParallelTargetCompileJob<'_>,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    manifest_runtime_options: &mut BTreeMap<PathBuf, ManifestRuntimeOptions>,
    driver_families: &mut DriverFamilyMap,
) -> Result<ParallelTargetCompileResult> {
    ensure_parent_dir(&job.compile_action.object_path)?;
    ensure_parent_dir(&job.compile_action.artifact_path)?;
    let compile_options = compile_action_options(
        command,
        job.compile_action,
        local_library_actions,
        built_std_packages,
        built_external_packages,
        manifest_runtime_options,
    )?;
    let mut summary = ExecutionSummary::default();
    let _ = build_compile_action_if_needed(
        job.compile_action,
        compile_options,
        driver_families,
        &mut summary,
        None,
    )?;

    Ok(ParallelTargetCompileResult {
        compile_object_path: job.compile_action.object_path.clone(),
        summary,
    })
}

pub(super) fn build_parallel_target_compile_jobs(
    command: crate::script::ScriptCommand,
    jobs: &[ParallelTargetCompileJob<'_>],
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<Vec<ParallelTargetCompileResult>> {
    let worker_count = target_parallel_worker_count(jobs.len());
    if worker_count <= 1 {
        return Ok(Vec::new());
    }

    let next_job = AtomicUsize::new(0);
    thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..worker_count {
            workers.push(scope.spawn(|| -> Result<Vec<ParallelTargetCompileResult>> {
                let mut results = Vec::new();
                let mut manifest_runtime_options = BTreeMap::new();
                let mut driver_families = DriverFamilyMap::new();
                loop {
                    let index = next_job.fetch_add(1, Ordering::Relaxed);
                    if index >= jobs.len() {
                        break;
                    }
                    results.push(build_parallel_target_compile_job(
                        command,
                        &jobs[index],
                        local_library_actions,
                        built_std_packages,
                        built_external_packages,
                        &mut manifest_runtime_options,
                        &mut driver_families,
                    )?);
                }
                Ok(results)
            }));
        }

        let mut results = Vec::new();
        for worker in workers {
            match worker.join() {
                Ok(Ok(mut worker_results)) => results.append(&mut worker_results),
                Ok(Err(err)) => return Err(err),
                Err(_) => {
                    return Err(Error::Execution(
                        "parallel target compile worker panicked".to_string(),
                    ));
                }
            }
        }
        Ok(results)
    })
}

pub(super) fn build_parallel_target_link_jobs(
    command: crate::script::ScriptCommand,
    jobs: &[ParallelTargetLinkJob<'_>],
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<Vec<ParallelTargetLinkResult>> {
    let worker_count = target_parallel_worker_count(jobs.len());
    if worker_count <= 1 {
        return Ok(Vec::new());
    }

    let next_job = AtomicUsize::new(0);
    thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..worker_count {
            workers.push(scope.spawn(|| -> Result<Vec<ParallelTargetLinkResult>> {
                let mut results = Vec::new();
                let mut manifest_runtime_options = BTreeMap::new();
                let mut driver_families = DriverFamilyMap::new();
                loop {
                    let index = next_job.fetch_add(1, Ordering::Relaxed);
                    if index >= jobs.len() {
                        break;
                    }
                    results.push(build_parallel_target_link_job(
                        command,
                        &jobs[index],
                        local_library_actions,
                        built_std_packages,
                        built_external_packages,
                        &mut manifest_runtime_options,
                        &mut driver_families,
                    )?);
                }
                Ok(results)
            }));
        }

        let mut results = Vec::new();
        for worker in workers {
            match worker.join() {
                Ok(Ok(mut worker_results)) => results.append(&mut worker_results),
                Ok(Err(err)) => return Err(err),
                Err(_) => {
                    return Err(Error::Execution(
                        "parallel target build worker panicked".to_string(),
                    ));
                }
            }
        }
        Ok(results)
    })
}
