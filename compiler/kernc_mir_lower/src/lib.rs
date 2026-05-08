#![doc = include_str!("../README.md")]

mod builder;
#[cfg(test)]
mod tests;

pub use builder::MirLowerError;
use kernc_mast::MastModule;

#[derive(Debug, Clone)]
pub struct MirBuildReport {
    pub module: kernc_mir::MirModule,
    pub workload: kernc_mir::MirWorkloadStats,
    pub summary: kernc_mir::MirSummaryIndex,
    pub pass_pipeline: kernc_mir::MirPassPipelineReport,
}

pub fn build_from_mast(module: &MastModule) -> MirBuildReport {
    try_build_from_mast(module).unwrap_or_else(|error| {
        panic!(
            "Kern ICE (MIR Lower): failed to lower MAST at {:?}: {}",
            error.span, error.message
        )
    })
}

pub fn try_build_from_mast(module: &MastModule) -> Result<MirBuildReport, MirLowerError> {
    let mut report = try_build_from_mast_unoptimized(module)?;
    report.pass_pipeline = kernc_mir::run_default_pass_pipeline(&mut report.module);
    kernc_mir::verify_module(&report.module)
        .expect("Kern ICE (MIR): pass pipeline built invalid MIR.");
    report.workload = report.module.workload_stats();
    report.summary = report.module.summary_index();
    Ok(report)
}

pub fn build_from_mast_unoptimized(module: &MastModule) -> MirBuildReport {
    try_build_from_mast_unoptimized(module).unwrap_or_else(|error| {
        panic!(
            "Kern ICE (MIR Lower): failed to lower MAST at {:?}: {}",
            error.span, error.message
        )
    })
}

pub fn try_build_from_mast_unoptimized(
    module: &MastModule,
) -> Result<MirBuildReport, MirLowerError> {
    builder::build_from_mast_unoptimized(module)
}
