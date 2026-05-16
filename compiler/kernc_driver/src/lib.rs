#![doc = include_str!("../README.md")]

mod compiler;
mod doc;
mod frontend;
mod language;
mod loader;
mod metadata;

pub use compiler::CompilerDriver;
pub use compiler::{
    AnalysisArtifact, AnalysisCall, AnalysisCallKind, AnalysisCompletionItem,
    AnalysisCompletionKind, AnalysisDeadStore, AnalysisDeadStoreKind, AnalysisDefinitionLink,
    AnalysisDocumentLink, AnalysisFlowBinding, AnalysisFlowBindingId, AnalysisFlowBindingKind,
    AnalysisFlowBindingSummary, AnalysisFlowCfg, AnalysisFlowCfgEdge, AnalysisFlowCfgEdgeKind,
    AnalysisFlowCfgNode, AnalysisFlowCfgNodeKind, AnalysisFlowDefUse, AnalysisFlowDefinitionFacts,
    AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef, AnalysisFlowLiveness,
    AnalysisFlowNodeEffects, AnalysisFlowNodeFacts, AnalysisFlowNodeId, AnalysisFlowNodeTransfer,
    AnalysisFlowOwner, AnalysisFlowOwnerKind, AnalysisFlowReaching, AnalysisFlowRegion,
    AnalysisFlowRegionKind, AnalysisFlowResolvedUse, AnalysisFlowResolvedUseKind,
    AnalysisFlowSingleSourceUse, AnalysisFlowSummary, AnalysisFlowUseDef, AnalysisHover,
    AnalysisNavigationArtifact, AnalysisOutline, AnalysisParameterInformation, AnalysisReference,
    AnalysisReport, AnalysisSemanticEntry, AnalysisSemanticKind, AnalysisSemanticRole,
    AnalysisSignatureHelp, AnalysisSignatureInformation, AnalysisSpanReplacement,
    AnalysisSurfaceArtifact, AnalysisSymbol, AnalysisSymbolKind, AnalysisTypeHint,
    AnalysisTypeHintKind, AnalysisUnusedBinding, AnalysisUnusedBindingKind, AnalysisUnusedItem,
    AnalysisUnusedItemKind, Canceled, CancellationToken, CodegenImportPlanReport,
    CodegenPlanFallback, CodegenPlanReport, CompileCacheStats, CompileReport,
    ImportedStructureArtifact, IncrementalDriverKey, ParsedModuleArtifact, PhaseTiming,
    SourceOverrides, StructureArtifact, TargetedAnalysisReport,
};
pub use doc::{KernDoc, KernDocEntry, KernDocSection, KernDocSectionKind, KmetaDocItem};
pub use metadata::{
    KMETA_DOCS_FILE, KMETA_MANIFEST_FILE, KmetaManifest, load_docs as load_kmeta_docs,
    load_manifest as load_kmeta_manifest,
};
