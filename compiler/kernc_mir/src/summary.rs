//! Cross-item reference summaries for MIR.
//!
//! The driver builds these summaries before partitioning codegen units. They
//! capture direct references, direct callsite counts, body availability, and a
//! small workload estimate for functions and globals.

use crate::{
    MirCallTarget, MirConst, MirFunction, MirGlobal, MirInlineAsm, MirInlineHint, MirInstruction,
    MirLinkage, MirMemoryIntrinsic, MirModule, MirOperand, MirPlace, MirRvalue, MirSliceBase,
    MirStaticInit, MirTerminator,
};
use kernc_mono::MonoId;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirItemBodyRole {
    DeclarationOnly,
    ExportRoot,
    InternalBody,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirReferenceSummary {
    pub function_ids: Vec<MonoId>,
    pub global_ids: Vec<MonoId>,
    pub direct_callee_ids: Vec<MonoId>,
    pub direct_callee_callsite_counts: Vec<MirDirectCalleeCallsiteCount>,
}

impl MirReferenceSummary {
    pub fn direct_callsite_count(&self, callee_id: MonoId) -> usize {
        self.direct_callee_callsite_counts
            .iter()
            .find(|entry| entry.callee_id == callee_id)
            .map(|entry| entry.callsite_count)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirDirectCalleeCallsiteCount {
    pub callee_id: MonoId,
    pub callsite_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunctionSummary {
    pub id: MonoId,
    pub name: String,
    pub linkage: MirLinkage,
    pub inline_hint: MirInlineHint,
    pub is_extern: bool,
    pub contains_control_flow_asm: bool,
    pub body_role: MirItemBodyRole,
    pub can_import_body: bool,
    pub param_count: usize,
    pub local_count: usize,
    pub block_count: usize,
    pub instruction_count: usize,
    pub direct_call_count: usize,
    pub indirect_call_count: usize,
    pub refs: MirReferenceSummary,
}

impl MirFunctionSummary {
    pub fn workload(&self) -> usize {
        (1 + self.block_count + self.instruction_count + self.direct_call_count).max(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirGlobalSummary {
    pub id: MonoId,
    pub name: String,
    pub linkage: MirLinkage,
    pub is_extern: bool,
    pub body_role: MirItemBodyRole,
    pub can_import_body: bool,
    pub init_node_count: usize,
    pub refs: MirReferenceSummary,
}

impl MirGlobalSummary {
    pub fn workload(&self) -> usize {
        (1 + self.init_node_count).max(1)
    }
}

#[derive(Debug, Clone, Default)]
pub struct MirSummaryIndex {
    pub functions: Vec<MirFunctionSummary>,
    pub globals: Vec<MirGlobalSummary>,
    pub callers_by_callee: HashMap<MonoId, Vec<MonoId>>,
    pub direct_callsites_by_callee: HashMap<MonoId, usize>,
    function_positions: HashMap<MonoId, usize>,
    global_positions: HashMap<MonoId, usize>,
}

impl MirSummaryIndex {
    pub fn function(&self, id: MonoId) -> Option<&MirFunctionSummary> {
        self.function_positions
            .get(&id)
            .map(|index| &self.functions[*index])
    }

    pub fn global(&self, id: MonoId) -> Option<&MirGlobalSummary> {
        self.global_positions
            .get(&id)
            .map(|index| &self.globals[*index])
    }
}

impl MirModule {
    pub fn summary_index(&self) -> MirSummaryIndex {
        let mut functions = self
            .functions
            .iter()
            .map(summarize_function)
            .collect::<Vec<_>>();
        let mut globals = self
            .globals
            .iter()
            .map(summarize_global)
            .collect::<Vec<_>>();
        functions.sort_by_key(|summary| summary.id);
        globals.sort_by_key(|summary| summary.id);

        let mut callers_by_callee = HashMap::<MonoId, Vec<MonoId>>::new();
        let mut direct_callsites_by_callee = HashMap::<MonoId, usize>::new();
        for function in &functions {
            for callee in &function.refs.direct_callee_ids {
                callers_by_callee
                    .entry(*callee)
                    .or_default()
                    .push(function.id);
            }
            for callsite_count in &function.refs.direct_callee_callsite_counts {
                *direct_callsites_by_callee
                    .entry(callsite_count.callee_id)
                    .or_default() += callsite_count.callsite_count;
            }
        }
        for callers in callers_by_callee.values_mut() {
            callers.sort();
            callers.dedup();
        }

        let function_positions = functions
            .iter()
            .enumerate()
            .map(|(index, summary)| (summary.id, index))
            .collect();
        let global_positions = globals
            .iter()
            .enumerate()
            .map(|(index, summary)| (summary.id, index))
            .collect();

        MirSummaryIndex {
            functions,
            globals,
            callers_by_callee,
            direct_callsites_by_callee,
            function_positions,
            global_positions,
        }
    }
}

fn summarize_function(function: &MirFunction) -> MirFunctionSummary {
    let (body_role, can_import_body) =
        summarize_body_role(function.body.is_some(), function.linkage);
    let mut refs = RefCollector::default();
    let mut instruction_count = 0;
    let mut block_count = 0;
    let mut contains_control_flow_asm = false;

    if let Some(body) = &function.body {
        block_count = body.blocks.len();
        instruction_count = body
            .blocks
            .iter()
            .map(|block| block.instructions.len())
            .sum();
        for block in &body.blocks {
            for instruction in &block.instructions {
                contains_control_flow_asm |= matches!(
                    &instruction.kind,
                    MirInstruction::InlineAsm(asm) if inline_asm_has_control_flow(asm)
                );
                refs.visit_instruction(&instruction.kind);
            }
            refs.visit_terminator(&block.terminator.kind);
        }
    }

    MirFunctionSummary {
        id: function.id,
        name: function.name.clone(),
        linkage: function.linkage,
        inline_hint: function.inline_hint,
        is_extern: function.is_extern,
        contains_control_flow_asm,
        body_role,
        can_import_body,
        param_count: function.params.len(),
        local_count: function
            .body
            .as_ref()
            .map(|body| body.locals.len())
            .unwrap_or_default(),
        block_count,
        instruction_count,
        direct_call_count: refs.direct_call_count,
        indirect_call_count: refs.indirect_call_count,
        refs: refs.finish(),
    }
}

fn summarize_global(global: &MirGlobal) -> MirGlobalSummary {
    let (body_role, can_import_body) = summarize_body_role(global.init.is_some(), global.linkage);
    let mut refs = RefCollector::default();
    let init_node_count = global
        .init
        .as_ref()
        .map(|init| refs.visit_static_init(init))
        .unwrap_or_default();

    MirGlobalSummary {
        id: global.id,
        name: global.name.clone(),
        linkage: global.linkage,
        is_extern: global.is_extern,
        body_role,
        can_import_body,
        init_node_count,
        refs: refs.finish(),
    }
}

fn summarize_body_role(has_body: bool, linkage: MirLinkage) -> (MirItemBodyRole, bool) {
    if !has_body {
        return (MirItemBodyRole::DeclarationOnly, false);
    }
    match linkage {
        MirLinkage::External => (MirItemBodyRole::ExportRoot, true),
        MirLinkage::LinkOnceOdr | MirLinkage::Internal => (MirItemBodyRole::InternalBody, true),
    }
}

fn inline_asm_has_control_flow(asm: &MirInlineAsm) -> bool {
    asm.asm_template
        .lines()
        .any(inline_asm_line_has_control_flow)
}

fn inline_asm_line_has_control_flow(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.ends_with(':') {
        return true;
    }
    let Some(opcode) = trimmed
        .split_whitespace()
        .next()
        .map(|token| token.trim_end_matches(','))
    else {
        return false;
    };
    opcode.eq_ignore_ascii_case("call")
        || opcode.eq_ignore_ascii_case("ret")
        || opcode.eq_ignore_ascii_case("loop")
        || opcode.eq_ignore_ascii_case("loope")
        || opcode.eq_ignore_ascii_case("loopne")
        || opcode.eq_ignore_ascii_case("loopnz")
        || opcode.eq_ignore_ascii_case("loopz")
        || opcode.starts_with('j')
        || opcode.starts_with('J')
}

#[derive(Default)]
struct RefCollector {
    function_ids: HashSet<MonoId>,
    global_ids: HashSet<MonoId>,
    direct_callee_counts: HashMap<MonoId, usize>,
    direct_call_count: usize,
    indirect_call_count: usize,
}

impl RefCollector {
    fn finish(self) -> MirReferenceSummary {
        let mut function_ids = self.function_ids.into_iter().collect::<Vec<_>>();
        let mut global_ids = self.global_ids.into_iter().collect::<Vec<_>>();
        let mut direct_callee_callsite_counts = self
            .direct_callee_counts
            .into_iter()
            .map(|(callee_id, callsite_count)| MirDirectCalleeCallsiteCount {
                callee_id,
                callsite_count,
            })
            .collect::<Vec<_>>();
        function_ids.sort();
        global_ids.sort();
        direct_callee_callsite_counts.sort_by_key(|entry| entry.callee_id);
        let direct_callee_ids = direct_callee_callsite_counts
            .iter()
            .map(|entry| entry.callee_id)
            .collect::<Vec<_>>();
        MirReferenceSummary {
            function_ids,
            global_ids,
            direct_callee_ids,
            direct_callee_callsite_counts,
        }
    }

    fn visit_static_init(&mut self, init: &MirStaticInit) -> usize {
        match init {
            MirStaticInit::Const(value) => {
                self.visit_const(value);
                1
            }
            MirStaticInit::Array { elems, .. } => {
                1 + elems
                    .iter()
                    .map(|elem| self.visit_static_init(elem))
                    .sum::<usize>()
            }
            MirStaticInit::FatPointer { data_ptr, meta, .. } => {
                1 + self.visit_static_init(data_ptr) + self.visit_static_init(meta)
            }
            MirStaticInit::Struct { fields, .. } => {
                1 + fields
                    .iter()
                    .map(|field| self.visit_static_init(field))
                    .sum::<usize>()
            }
            MirStaticInit::Union { value, .. } => 1 + self.visit_static_init(value),
            MirStaticInit::Data { payload, .. } => {
                1 + payload
                    .as_deref()
                    .map(|value| self.visit_static_init(value))
                    .unwrap_or_default()
            }
        }
    }

    fn visit_instruction(&mut self, instruction: &MirInstruction) {
        match instruction {
            MirInstruction::Let { place, init } => {
                self.visit_place(place);
                self.visit_rvalue(init);
            }
            MirInstruction::Assign { place, value, .. } => {
                self.visit_place(place);
                self.visit_rvalue(value);
            }
            MirInstruction::Memory(memory) => self.visit_memory(memory),
            MirInstruction::InlineAsm(asm) => self.visit_inline_asm(asm),
            MirInstruction::SimdStore { ptr, value, .. } => {
                self.visit_operand(ptr);
                self.visit_operand(value);
            }
            MirInstruction::SimdMaskedStore {
                ptr, mask, value, ..
            } => {
                self.visit_operand(ptr);
                self.visit_operand(mask);
                self.visit_operand(value);
            }
            MirInstruction::SimdScatter {
                ptr,
                indices,
                value,
            } => {
                self.visit_operand(ptr);
                self.visit_operand(indices);
                self.visit_operand(value);
            }
            MirInstruction::SimdMaskedScatter {
                ptr,
                indices,
                mask,
                value,
            } => {
                self.visit_operand(ptr);
                self.visit_operand(indices);
                self.visit_operand(mask);
                self.visit_operand(value);
            }
            MirInstruction::AtomicStore { ptr, value, .. } => {
                self.visit_operand(ptr);
                self.visit_operand(value);
            }
            MirInstruction::Fence { .. } | MirInstruction::Trap | MirInstruction::Breakpoint => {}
            MirInstruction::Eval(rvalue) | MirInstruction::Defer(rvalue) => {
                self.visit_rvalue(rvalue)
            }
        }
    }

    fn visit_terminator(&mut self, terminator: &MirTerminator) {
        match terminator {
            MirTerminator::Goto(_) | MirTerminator::Unreachable => {}
            MirTerminator::Branch { cond, .. } => self.visit_rvalue(cond),
            MirTerminator::Switch { target, .. } => self.visit_rvalue(target),
            MirTerminator::Return(value) => {
                if let Some(value) = value {
                    self.visit_rvalue(value);
                }
            }
        }
    }

    fn visit_memory(&mut self, memory: &MirMemoryIntrinsic) {
        match memory {
            MirMemoryIntrinsic::Copy { dest, src, len }
            | MirMemoryIntrinsic::Move { dest, src, len } => {
                self.visit_operand(dest);
                self.visit_operand(src);
                self.visit_operand(len);
            }
            MirMemoryIntrinsic::Set { dest, val, len } => {
                self.visit_operand(dest);
                self.visit_operand(val);
                self.visit_operand(len);
            }
        }
    }

    fn visit_inline_asm(&mut self, asm: &MirInlineAsm) {
        for operand in &asm.input_args {
            self.visit_operand(operand);
        }
        for operand in &asm.output_ptrs {
            self.visit_operand(operand);
        }
    }

    fn visit_rvalue(&mut self, rvalue: &MirRvalue) {
        match rvalue {
            MirRvalue::Use(operand)
            | MirRvalue::Projection { operand, .. }
            | MirRvalue::Unary { operand, .. }
            | MirRvalue::Cast { operand, .. }
            | MirRvalue::BitIntrinsic { operand, .. }
            | MirRvalue::AtomicLoad { ptr: operand, .. }
            | MirRvalue::SimdUnaryIntrinsic { operand, .. }
            | MirRvalue::SimdReduce { operand, .. }
            | MirRvalue::SimdAny { operand }
            | MirRvalue::SimdAll { operand }
            | MirRvalue::SimdBitmask { operand }
            | MirRvalue::SimdSplat { value: operand }
            | MirRvalue::SimdCast { value: operand }
            | MirRvalue::SimdBitcast { value: operand }
            | MirRvalue::SimdLoad { ptr: operand, .. } => self.visit_operand(operand),
            MirRvalue::Call { callee, args } => {
                match callee {
                    MirCallTarget::Direct(id) => {
                        self.direct_call_count += 1;
                        *self.direct_callee_counts.entry(*id).or_default() += 1;
                        self.function_ids.insert(*id);
                    }
                    MirCallTarget::Operand(operand) => {
                        self.indirect_call_count += 1;
                        self.visit_operand(operand);
                    }
                }
                for arg in args {
                    self.visit_operand(arg);
                }
            }
            MirRvalue::Aggregate { fields, .. } => {
                for field in fields {
                    self.visit_operand(field);
                }
            }
            MirRvalue::Binary { lhs, rhs, .. }
            | MirRvalue::AtomicRmw {
                ptr: lhs,
                value: rhs,
                ..
            }
            | MirRvalue::SimdBinaryIntrinsic { lhs, rhs, .. }
            | MirRvalue::SimdShuffle { lhs, rhs, .. }
            | MirRvalue::SimdInsertHalf {
                base: lhs,
                half: rhs,
                ..
            }
            | MirRvalue::SimdGather {
                ptr: lhs,
                indices: rhs,
            } => {
                self.visit_operand(lhs);
                self.visit_operand(rhs);
            }
            MirRvalue::AtomicCas {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.visit_operand(ptr);
                self.visit_operand(expected);
                self.visit_operand(desired);
            }
            MirRvalue::SimdMaskedLoad {
                ptr, mask, or_else, ..
            } => {
                self.visit_operand(ptr);
                self.visit_operand(mask);
                self.visit_operand(or_else);
            }
            MirRvalue::SimdMaskedGather {
                ptr,
                indices,
                mask,
                or_else,
            } => {
                self.visit_operand(ptr);
                self.visit_operand(indices);
                self.visit_operand(mask);
                self.visit_operand(or_else);
            }
            MirRvalue::SliceOp {
                lhs, start, end, ..
            } => {
                self.visit_slice_base(lhs);
                if let Some(start) = start {
                    self.visit_operand(start);
                }
                if let Some(end) = end {
                    self.visit_operand(end);
                }
            }
            MirRvalue::AddressOf(place) | MirRvalue::Load(place) => self.visit_place(place),
            MirRvalue::SimdSelect {
                mask,
                on_true,
                on_false,
            } => {
                self.visit_operand(mask);
                self.visit_operand(on_true);
                self.visit_operand(on_false);
            }
        }
    }

    fn visit_slice_base(&mut self, base: &MirSliceBase) {
        match base {
            MirSliceBase::Operand(operand) => self.visit_operand(operand),
            MirSliceBase::Place(place) => self.visit_place(place),
        }
    }

    fn visit_place(&mut self, place: &MirPlace) {
        match place {
            MirPlace::Local(_) => {}
            MirPlace::Global(id) => {
                self.global_ids.insert(*id);
            }
            MirPlace::Deref(operand) => self.visit_operand(operand),
            MirPlace::Field { base, .. } => self.visit_place(base),
            MirPlace::Index { base, index } => {
                self.visit_place(base);
                self.visit_operand(index);
            }
        }
    }

    fn visit_operand(&mut self, operand: &MirOperand) {
        match operand {
            MirOperand::Local(_) => {}
            MirOperand::Const(value) => self.visit_const(value),
        }
    }

    fn visit_const(&mut self, value: &MirConst) {
        match value {
            MirConst::GlobalRef { id, .. } => {
                self.global_ids.insert(*id);
            }
            MirConst::FuncRef { id, .. } => {
                self.function_ids.insert(*id);
            }
            MirConst::Undef { .. }
            | MirConst::Integer { .. }
            | MirConst::Float { .. }
            | MirConst::Bool { .. }
            | MirConst::StringLiteral { .. } => {}
        }
    }
}
