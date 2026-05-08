use crate::context::SemaContext;
use crate::def::DefId;
use crate::passes::TypeResolver;
use crate::scope::ScopeId;
use crate::ty::{
    AnonymousEnum, AnonymousVariant, BuiltinAnonymousEnumKind, ConstExprKind, ConstGeneric,
    GenericArg, TypeId, TypeKind,
};
use kernc_ast::{self as ast, AssignmentOperator, Expr, ExprKind, UnaryOperator};
use kernc_utils::{FastHashMap, FastHashSet, NodeId, Span, SymbolId};
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::time::{Duration, Instant};

mod access;
mod call;
mod cast;
mod coercion;
mod control;
mod literal;
mod ops;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumericInferenceKind {
    IntLiteral,
    FloatLiteral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NumericInferenceState {
    pub(crate) kind: NumericInferenceKind,
    pub(crate) candidates: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PointerOrigin {
    Temporary(Span),
    Parameter(usize),
}

pub(crate) struct ExprChecker<'a, 'ctx> {
    pub(crate) ctx: &'a mut SemaContext<'ctx>,
    pub(crate) current_return_type: Option<TypeId>,
    pub(crate) has_returned: bool,
    pub(crate) type_vars: Vec<Option<TypeId>>,
    pub(crate) numeric_type_vars: Vec<Option<NumericInferenceState>>,
    pub(crate) trait_obligation_stack: Vec<(TypeId, TypeId)>,
    pub(crate) projection_normalization_stack: Vec<TypeId>,
    pub(crate) current_module_cache: Option<(ScopeId, Option<DefId>)>,
    pub(crate) allow_uninstantiated_generic_function_items: bool,
    pub(crate) touched_expr_nodes: Vec<NodeId>,
    pub(crate) touched_bindings: Vec<(ScopeId, SymbolId)>,
    pub(crate) pointer_origin_bindings:
        FastHashMap<(ScopeId, SymbolId), FastHashSet<PointerOrigin>>,
    pub(crate) pointer_origin_exprs: FastHashMap<NodeId, FastHashSet<PointerOrigin>>,
    pub(crate) stored_parameters: FastHashSet<usize>,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn string_literal_type(&mut self, value: &str) -> TypeId {
        self.ctx.type_registry.intern(TypeKind::Array {
            elem: TypeId::U8,
            len: ConstGeneric::Value(crate::ty::ConstGenericValue {
                ty: TypeId::USIZE,
                kind: crate::ty::ConstGenericValueKind::Int(value.len() as i128),
            }),
        })
    }

    const NUMERIC_CAND_I8: u16 = 1 << 0;
    const NUMERIC_CAND_I16: u16 = 1 << 1;
    const NUMERIC_CAND_I32: u16 = 1 << 2;
    const NUMERIC_CAND_I64: u16 = 1 << 3;
    const NUMERIC_CAND_I128: u16 = 1 << 4;
    const NUMERIC_CAND_ISIZE: u16 = 1 << 5;
    const NUMERIC_CAND_U8: u16 = 1 << 6;
    const NUMERIC_CAND_U16: u16 = 1 << 7;
    const NUMERIC_CAND_U32: u16 = 1 << 8;
    const NUMERIC_CAND_U64: u16 = 1 << 9;
    const NUMERIC_CAND_U128: u16 = 1 << 10;
    const NUMERIC_CAND_USIZE: u16 = 1 << 11;
    const NUMERIC_CAND_F32: u16 = 1 << 12;
    const NUMERIC_CAND_F64: u16 = 1 << 13;

    const NUMERIC_CAND_ALL_INTS: u16 = Self::NUMERIC_CAND_I8
        | Self::NUMERIC_CAND_I16
        | Self::NUMERIC_CAND_I32
        | Self::NUMERIC_CAND_I64
        | Self::NUMERIC_CAND_I128
        | Self::NUMERIC_CAND_ISIZE
        | Self::NUMERIC_CAND_U8
        | Self::NUMERIC_CAND_U16
        | Self::NUMERIC_CAND_U32
        | Self::NUMERIC_CAND_U64
        | Self::NUMERIC_CAND_U128
        | Self::NUMERIC_CAND_USIZE;
    const NUMERIC_CAND_ALL_FLOATS: u16 = Self::NUMERIC_CAND_F32 | Self::NUMERIC_CAND_F64;
    const NUMERIC_CAND_ALL: u16 = Self::NUMERIC_CAND_ALL_INTS | Self::NUMERIC_CAND_ALL_FLOATS;
    const NUMERIC_CAND_POINTER_OFFSETS: u16 = Self::NUMERIC_CAND_ISIZE | Self::NUMERIC_CAND_USIZE;

    pub(crate) fn new(ctx: &'a mut SemaContext<'ctx>, current_return_type: Option<TypeId>) -> Self {
        Self {
            ctx,
            current_return_type,
            has_returned: false,
            type_vars: Vec::new(),
            numeric_type_vars: Vec::new(),
            trait_obligation_stack: Vec::new(),
            projection_normalization_stack: Vec::new(),
            current_module_cache: None,
            allow_uninstantiated_generic_function_items: false,
            touched_expr_nodes: Vec::new(),
            touched_bindings: Vec::new(),
            pointer_origin_bindings: FastHashMap::default(),
            pointer_origin_exprs: FastHashMap::default(),
            stored_parameters: FastHashSet::default(),
        }
    }

    pub(crate) fn numeric_state_for_kind(kind: NumericInferenceKind) -> NumericInferenceState {
        let candidates = match kind {
            NumericInferenceKind::IntLiteral => Self::NUMERIC_CAND_ALL,
            NumericInferenceKind::FloatLiteral => Self::NUMERIC_CAND_ALL_FLOATS,
        };
        NumericInferenceState { kind, candidates }
    }

    pub(crate) fn pointer_origins(&self, expr: &Expr) -> FastHashSet<PointerOrigin> {
        let mut origins = FastHashSet::default();
        self.collect_pointer_origins(expr, &mut origins);
        origins
    }

    fn collect_pointer_origins(&self, expr: &Expr, out: &mut FastHashSet<PointerOrigin>) {
        if let Some(expr_origins) = self.pointer_origin_exprs.get(&expr.id) {
            out.extend(expr_origins);
        }

        match &expr.kind {
            ExprKind::Grouped { expr: inner } => self.collect_pointer_origins(inner, out),
            ExprKind::Unary {
                op: UnaryOperator::MutAddressOf,
                operand,
            } if self.can_materialize_mut_temporary(operand) => {
                out.insert(PointerOrigin::Temporary(expr.span));
            }
            ExprKind::Unary {
                op: UnaryOperator::PointerDeRef,
                ..
            } => {}
            ExprKind::Unary { operand, .. }
            | ExprKind::Propagate { operand, .. }
            | ExprKind::Defer { expr: operand }
            | ExprKind::As { lhs: operand, .. } => self.collect_pointer_origins(operand, out),
            ExprKind::GenericInstantiation { target, .. } => {
                self.collect_pointer_origins(target, out)
            }
            ExprKind::Binary { .. }
            | ExprKind::Assign { .. }
            | ExprKind::FieldAccess { .. }
            | ExprKind::IndexAccess { .. } => {}
            ExprKind::Call { .. } => {}
            ExprKind::DataInit { literal, .. } => {
                self.collect_pointer_origins_in_literal(literal, out);
            }
            ExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.collect_pointer_origins(then_branch, out);
                if let Some(else_branch) = else_branch {
                    self.collect_pointer_origins(else_branch, out);
                }
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    self.collect_pointer_origins(&arm.body, out);
                }
            }
            ExprKind::Block { result, .. } => {
                if let Some(result) = result {
                    self.collect_pointer_origins(result, out);
                }
            }
            ExprKind::While { .. } => {}
            ExprKind::SliceOp { .. } => {}
            ExprKind::Return(value) => {
                if let Some(value) = value {
                    self.collect_pointer_origins(value, out);
                }
            }
            ExprKind::Let { init, .. } | ExprKind::Static { init, .. } => {
                self.collect_pointer_origins(init, out);
            }
            ExprKind::Closure { captures, .. } => {
                for capture in captures {
                    self.collect_pointer_origins(&capture.value, out);
                }
            }
            ExprKind::Identifier(_)
            | ExprKind::Error
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::String(_)
            | ExprKind::AnchoredPath { .. }
            | ExprKind::TypeNode(_)
            | ExprKind::EnumLiteral { .. }
            | ExprKind::Break
            | ExprKind::Continue
            | ExprKind::Undef
            | ExprKind::Infer
            | ExprKind::SelfValue => {
                self.collect_direct_pointer_origins(expr, out);
            }
        }
    }

    fn collect_pointer_origins_in_literal(
        &self,
        literal: &ast::DataLiteralKind,
        out: &mut FastHashSet<PointerOrigin>,
    ) {
        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    self.collect_pointer_origins(&field.value, out);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    self.collect_pointer_origins(item, out);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.collect_pointer_origins(value, out);
                self.collect_pointer_origins(count, out);
            }
            ast::DataLiteralKind::Scalar(value) => self.collect_pointer_origins(value, out),
        }
    }

    fn collect_direct_pointer_origins(&self, expr: &Expr, out: &mut FastHashSet<PointerOrigin>) {
        match &expr.kind {
            ExprKind::Grouped { expr } => self.collect_direct_pointer_origins(expr, out),
            ExprKind::Identifier(name) => {
                if let Some(origins) = self.pointer_origin_binding_set(*name) {
                    out.extend(origins);
                }
            }
            ExprKind::As { lhs, .. } => self.collect_direct_pointer_origins(lhs, out),
            _ => {}
        }
    }

    fn pointer_origin_binding_set(&self, name: SymbolId) -> Option<FastHashSet<PointerOrigin>> {
        let mut curr = self.ctx.scopes.current_scope_id();
        while let Some(scope_id) = curr {
            if let Some(origins) = self.pointer_origin_bindings.get(&(scope_id, name)) {
                return Some(origins.clone());
            }
            curr = self.ctx.scopes.parent_scope_id(scope_id);
        }
        None
    }

    pub(crate) fn record_pointer_origin_binding(
        &mut self,
        name: SymbolId,
        origins: FastHashSet<PointerOrigin>,
    ) {
        if origins.is_empty() {
            return;
        }
        let Some(scope_id) = self.ctx.scopes.current_scope_id() else {
            return;
        };
        self.pointer_origin_bindings
            .insert((scope_id, name), origins);
    }

    pub(crate) fn record_pointer_origin_expr(&mut self, node_id: NodeId, origin: PointerOrigin) {
        self.pointer_origin_exprs
            .entry(node_id)
            .or_default()
            .insert(origin);
    }

    pub(crate) fn record_parameter_binding(&mut self, name: SymbolId, param_index: usize) {
        let mut origins = FastHashSet::default();
        origins.insert(PointerOrigin::Parameter(param_index));
        self.record_pointer_origin_binding(name, origins);
    }

    pub(crate) fn record_pointer_origins_from_pattern(
        &mut self,
        pattern: &ast::Pattern,
        init: &Expr,
    ) {
        self.record_pointer_origin_pattern_bindings_from_expr(pattern, init);
    }

    fn record_pointer_origin_pattern_bindings_from_expr(
        &mut self,
        pattern: &ast::Pattern,
        init: &Expr,
    ) {
        let origins = self.pointer_origins(init);
        self.record_pointer_origin_pattern_bindings(pattern, init, &origins);
    }

    fn record_pointer_origin_pattern_bindings(
        &mut self,
        pattern: &ast::Pattern,
        init: &Expr,
        origins: &FastHashSet<PointerOrigin>,
    ) {
        match &pattern.kind {
            ast::PatternKind::Binding(binding) => {
                if self.ctx.resolve(binding.name) != "_" {
                    self.record_pointer_origin_binding(binding.name, origins.clone());
                }
            }
            ast::PatternKind::Destructure(destructure) => {
                if let ExprKind::DataInit {
                    literal: ast::DataLiteralKind::Struct(init_fields),
                    ..
                } = &init.kind
                {
                    for field in &destructure.fields {
                        if let Some(init_field) = init_fields
                            .iter()
                            .find(|init_field| init_field.name == field.name)
                        {
                            self.record_pointer_origin_pattern_bindings_from_expr(
                                &field.pattern,
                                &init_field.value,
                            );
                        }
                    }
                    return;
                }
                for field in &destructure.fields {
                    self.record_pointer_origin_pattern_bindings(&field.pattern, init, origins);
                }
            }
            ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {}
        }
    }

    pub(crate) fn reject_temporary_address_escape(&mut self, expr: &Expr, destination: &str) {
        let origins = self.pointer_origins(expr);
        self.reject_temporary_origins(&origins, destination);
        if destination == "static storage" {
            self.record_parameter_store_from_origins(&origins);
        }
    }

    fn reject_temporary_origins(
        &mut self,
        origins: &FastHashSet<PointerOrigin>,
        destination: &str,
    ) {
        for origin in origins {
            if let PointerOrigin::Temporary(address_span) = origin {
                self.emit_temporary_address_escape(*address_span, destination);
            }
        }
    }

    fn emit_temporary_address_escape(&mut self, address_span: Span, destination: &str) {
        self.ctx
            .struct_error(
                address_span,
                format!("address of temporary value escapes into {}", destination),
            )
            .with_hint("`..&` may materialize a temporary that is only valid in the current scope")
            .with_hint("bind the value to stable storage before taking its address")
            .emit();
    }

    fn record_parameter_store_from_origins(&mut self, origins: &FastHashSet<PointerOrigin>) {
        for origin in origins {
            if let PointerOrigin::Parameter(index) = origin {
                self.stored_parameters.insert(*index);
            }
        }
    }

    pub(crate) fn assignment_targets_static_storage(&self, lhs: &Expr) -> bool {
        match &lhs.kind {
            ExprKind::Grouped { expr } => self.assignment_targets_static_storage(expr),
            ExprKind::Identifier(name) => self
                .ctx
                .scopes
                .resolve_value_symbol(*name)
                .is_some_and(|info| info.kind == crate::scope::SymbolKind::Static),
            ExprKind::FieldAccess { lhs, .. }
            | ExprKind::IndexAccess { lhs, .. }
            | ExprKind::SliceOp { lhs, .. } => self.assignment_targets_static_storage(lhs),
            ExprKind::Unary {
                op: UnaryOperator::PointerDeRef,
                operand,
            } => self.assignment_targets_static_storage(operand),
            _ => false,
        }
    }

    pub(crate) fn assignment_may_store_long_lived_pointer(
        &self,
        lhs: &Expr,
        op: AssignmentOperator,
    ) -> bool {
        op == AssignmentOperator::Assign && self.assignment_targets_static_storage(lhs)
    }

    pub(crate) fn numeric_candidates_for_type(ty: TypeId) -> u16 {
        match ty {
            TypeId::I8 => Self::NUMERIC_CAND_I8,
            TypeId::I16 => Self::NUMERIC_CAND_I16,
            TypeId::I32 => Self::NUMERIC_CAND_I32,
            TypeId::I64 => Self::NUMERIC_CAND_I64,
            TypeId::I128 => Self::NUMERIC_CAND_I128,
            TypeId::ISIZE => Self::NUMERIC_CAND_ISIZE,
            TypeId::U8 => Self::NUMERIC_CAND_U8,
            TypeId::U16 => Self::NUMERIC_CAND_U16,
            TypeId::U32 => Self::NUMERIC_CAND_U32,
            TypeId::U64 => Self::NUMERIC_CAND_U64,
            TypeId::U128 => Self::NUMERIC_CAND_U128,
            TypeId::USIZE => Self::NUMERIC_CAND_USIZE,
            TypeId::F32 => Self::NUMERIC_CAND_F32,
            TypeId::F64 => Self::NUMERIC_CAND_F64,
            _ => 0,
        }
    }

    pub(crate) fn numeric_candidates_have_integers(candidates: u16) -> bool {
        candidates & Self::NUMERIC_CAND_ALL_INTS != 0
    }

    pub(crate) fn numeric_candidates_have_floats(candidates: u16) -> bool {
        candidates & Self::NUMERIC_CAND_ALL_FLOATS != 0
    }

    pub(crate) fn single_numeric_candidate_type(candidates: u16) -> Option<TypeId> {
        [
            (Self::NUMERIC_CAND_I8, TypeId::I8),
            (Self::NUMERIC_CAND_I16, TypeId::I16),
            (Self::NUMERIC_CAND_I32, TypeId::I32),
            (Self::NUMERIC_CAND_I64, TypeId::I64),
            (Self::NUMERIC_CAND_I128, TypeId::I128),
            (Self::NUMERIC_CAND_ISIZE, TypeId::ISIZE),
            (Self::NUMERIC_CAND_U8, TypeId::U8),
            (Self::NUMERIC_CAND_U16, TypeId::U16),
            (Self::NUMERIC_CAND_U32, TypeId::U32),
            (Self::NUMERIC_CAND_U64, TypeId::U64),
            (Self::NUMERIC_CAND_U128, TypeId::U128),
            (Self::NUMERIC_CAND_USIZE, TypeId::USIZE),
            (Self::NUMERIC_CAND_F32, TypeId::F32),
            (Self::NUMERIC_CAND_F64, TypeId::F64),
        ]
        .into_iter()
        .find_map(|(mask, ty)| (candidates == mask).then_some(ty))
    }

    pub(crate) fn finalize_numeric_inference(&mut self, ty: TypeId) -> TypeId {
        let final_ty = self.materialize_numeric_defaults_in_type(ty);

        let touched_expr_nodes = self.touched_expr_nodes.clone();
        for node_id in touched_expr_nodes {
            let Some(existing_ty) = self.ctx.node_type(node_id) else {
                continue;
            };
            let rewritten = self.materialize_numeric_defaults_in_type(existing_ty);
            self.ctx.set_node_type(node_id, rewritten);
        }

        let touched_bindings = self.touched_bindings.clone();
        for (scope_id, name) in touched_bindings {
            let Some(existing_ty) = self
                .ctx
                .scopes
                .resolve_in(scope_id, name)
                .map(|info| info.type_id)
            else {
                continue;
            };
            let rewritten = self.materialize_numeric_defaults_in_type(existing_ty);
            let _ = self
                .ctx
                .scopes
                .update_type_in_scope(scope_id, name, rewritten);
        }

        final_ty
    }

    pub(crate) fn maybe_constrain_by_expected_type(
        &mut self,
        actual: TypeId,
        expected: Option<TypeId>,
    ) -> TypeId {
        let Some(expected) = expected else {
            return actual;
        };

        let resolved_actual = self.resolve_tv(actual);
        let TypeKind::TypeVar(vid) = self.ctx.type_registry.get(resolved_actual).clone() else {
            return actual;
        };
        if self.numeric_inference_kind(vid).is_none() {
            return actual;
        }

        if self.bind_type_var(vid, expected) {
            self.resolve_tv(actual)
        } else {
            actual
        }
    }

    pub(crate) fn record_current_binding(&mut self, name: SymbolId) {
        let Some(scope_id) = self.ctx.scopes.current_scope_id() else {
            return;
        };
        self.touched_bindings.push((scope_id, name));
    }

    pub(crate) fn numeric_inference_kind(&self, vid: u32) -> Option<NumericInferenceKind> {
        self.numeric_type_vars
            .get(vid as usize)
            .copied()
            .flatten()
            .map(|state| state.kind)
    }

    pub(crate) fn numeric_inference_state(&self, vid: u32) -> Option<NumericInferenceState> {
        self.numeric_type_vars.get(vid as usize).copied().flatten()
    }

    pub(crate) fn type_numeric_candidates(&self, ty: TypeId) -> Option<u16> {
        match self.ctx.type_registry.get(ty) {
            TypeKind::TypeVar(vid) => self
                .numeric_inference_state(*vid)
                .map(|state| state.candidates),
            _ => None,
        }
    }

    pub(crate) fn type_is_integer_like(&mut self, ty: TypeId) -> bool {
        let norm = self.resolve_tv(ty);
        self.ctx.type_registry.is_integer(norm)
            || self
                .type_numeric_candidates(norm)
                .is_some_and(Self::numeric_candidates_have_integers)
    }

    pub(crate) fn type_is_float_like(&mut self, ty: TypeId) -> bool {
        let norm = self.resolve_tv(ty);
        self.ctx.type_registry.is_float(norm)
            || self
                .type_numeric_candidates(norm)
                .is_some_and(Self::numeric_candidates_have_floats)
    }

    pub(crate) fn type_is_numeric_like(&mut self, ty: TypeId) -> bool {
        self.type_is_integer_like(ty) || self.type_is_float_like(ty)
    }

    fn numeric_default_type(&self, candidates: u16) -> TypeId {
        [
            TypeId::I32,
            TypeId::F64,
            TypeId::ISIZE,
            TypeId::USIZE,
            TypeId::I64,
            TypeId::U32,
            TypeId::F32,
            TypeId::I128,
            TypeId::U64,
            TypeId::I16,
            TypeId::U128,
            TypeId::U16,
            TypeId::I8,
            TypeId::U8,
        ]
        .into_iter()
        .find(|ty| Self::numeric_candidates_for_type(*ty) & candidates != 0)
        .unwrap_or(TypeId::ERROR)
    }

    fn materialize_numeric_defaults_in_generic_arg(&mut self, arg: GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(self.materialize_numeric_defaults_in_type(ty)),
            GenericArg::Const(value) => GenericArg::Const(value),
        }
    }

    fn materialize_numeric_defaults_in_type(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.resolve_tv(ty);
        let kind = self.ctx.type_registry.get(resolved).clone();

        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::Param(_) => resolved,
            TypeKind::TypeVar(vid) => self
                .numeric_inference_state(vid)
                .map(|state| self.numeric_default_type(state.candidates))
                .unwrap_or(resolved),
            TypeKind::Pointer { is_mut, elem } => {
                let elem = self.materialize_numeric_defaults_in_type(elem);
                self.ctx
                    .type_registry
                    .intern(TypeKind::Pointer { is_mut, elem })
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let elem = self.materialize_numeric_defaults_in_type(elem);
                self.ctx
                    .type_registry
                    .intern(TypeKind::VolatilePtr { is_mut, elem })
            }
            TypeKind::Slice { is_mut, elem } => {
                let elem = self.materialize_numeric_defaults_in_type(elem);
                self.ctx
                    .type_registry
                    .intern(TypeKind::Slice { is_mut, elem })
            }
            TypeKind::Array { elem, len } => {
                let elem = self.materialize_numeric_defaults_in_type(elem);
                self.ctx.type_registry.intern(TypeKind::Array { elem, len })
            }
            TypeKind::ArrayInfer { elem } => {
                let elem = self.materialize_numeric_defaults_in_type(elem);
                self.ctx.type_registry.intern(TypeKind::ArrayInfer { elem })
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let params = params
                    .into_iter()
                    .map(|param| self.materialize_numeric_defaults_in_type(param))
                    .collect();
                let ret = self.materialize_numeric_defaults_in_type(ret);
                self.ctx.type_registry.intern(TypeKind::Function {
                    params,
                    ret,
                    is_variadic,
                })
            }
            TypeKind::Def(def_id, args) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Def(def_id, args))
            }
            TypeKind::Enum(def_id, args) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Enum(def_id, args))
            }
            TypeKind::EnumPayload(def_id, args) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::EnumPayload(def_id, args))
            }
            TypeKind::FnDef(def_id, args) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                self.ctx.type_registry.intern(TypeKind::FnDef(def_id, args))
            }
            TypeKind::Alias(name, target) => {
                let target = self.materialize_numeric_defaults_in_type(target);
                self.ctx.type_registry.intern(TypeKind::Alias(name, target))
            }
            TypeKind::Associated(def_id, args) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Associated(def_id, args))
            }
            TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                let assoc_bindings = assoc_bindings
                    .into_iter()
                    .map(|(assoc_def_id, assoc_ty)| {
                        (
                            assoc_def_id,
                            self.materialize_numeric_defaults_in_type(assoc_ty),
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, args, assoc_bindings))
            }
            TypeKind::Projection {
                target,
                trait_def_id,
                trait_args,
                assoc_def_id,
                assoc_args,
            } => {
                let target = self.materialize_numeric_defaults_in_type(target);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                let assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Projection {
                    target,
                    trait_def_id,
                    trait_args,
                    assoc_def_id,
                    assoc_args,
                })
            }
            TypeKind::ClosureInterface { params, ret } => {
                let params = params
                    .into_iter()
                    .map(|param| self.materialize_numeric_defaults_in_type(param))
                    .collect();
                let ret = self.materialize_numeric_defaults_in_type(ret);
                self.ctx
                    .type_registry
                    .intern(TypeKind::ClosureInterface { params, ret })
            }
            TypeKind::AnonymousState {
                closure_node_id,
                captures,
                params,
                ret,
            } => {
                let captures = captures
                    .into_iter()
                    .map(|capture| self.materialize_numeric_defaults_in_type(capture))
                    .collect();
                let params = params
                    .into_iter()
                    .map(|param| self.materialize_numeric_defaults_in_type(param))
                    .collect();
                let ret = self.materialize_numeric_defaults_in_type(ret);
                self.ctx.type_registry.intern(TypeKind::AnonymousState {
                    closure_node_id,
                    captures,
                    params,
                    ret,
                })
            }
            TypeKind::AnonymousStruct(is_extern, fields) => {
                let fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.materialize_numeric_defaults_in_type(field.ty),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousStruct(is_extern, fields))
            }
            TypeKind::AnonymousUnion(is_extern, fields) => {
                let fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.materialize_numeric_defaults_in_type(field.ty),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(is_extern, fields))
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let backing_ty = enum_def
                    .backing_ty
                    .map(|ty| self.materialize_numeric_defaults_in_type(ty));
                let variants = enum_def
                    .variants
                    .into_iter()
                    .map(|variant| AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty: variant
                            .payload_ty
                            .map(|ty| self.materialize_numeric_defaults_in_type(ty)),
                        explicit_value: variant.explicit_value,
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                        backing_ty,
                        builtin: enum_def.builtin,
                        variants,
                    }))
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let enum_ty = self.materialize_numeric_defaults_in_type(enum_ty);
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnumPayload(enum_ty))
            }
        }
    }

    pub(crate) fn format_type_for_diagnostic(&mut self, ty: TypeId) -> String {
        let resolved = self.resolve_tv(ty);
        let Some(candidates) = self.type_numeric_candidates(resolved) else {
            return self.ctx.ty_to_string(ty);
        };

        if let Some(single) = Self::single_numeric_candidate_type(candidates) {
            return self.ctx.ty_to_string(single);
        }

        let ints = Self::numeric_candidates_have_integers(candidates);
        let floats = Self::numeric_candidates_have_floats(candidates);
        if candidates == Self::NUMERIC_CAND_POINTER_OFFSETS {
            return "an inferred pointer offset integer (`usize` or `isize`)".to_string();
        }
        match (ints, floats) {
            (true, true) => "an inferred numeric literal".to_string(),
            (true, false) => "an inferred integer literal".to_string(),
            (false, true) => "an inferred floating-point literal".to_string(),
            (false, false) => self.ctx.ty_to_string(ty),
        }
    }

    pub(crate) fn with_uninstantiated_generic_function_items_allowed<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let prev = self.allow_uninstantiated_generic_function_items;
        self.allow_uninstantiated_generic_function_items = true;
        let result = f(self);
        self.allow_uninstantiated_generic_function_items = prev;
        result
    }

    fn reject_uninstantiated_generic_function_item(&mut self, expr: &Expr, ty: TypeId) -> TypeId {
        let norm_ty = self.resolve_tv(ty);
        let TypeKind::FnDef(def_id, generic_args) = self.ctx.type_registry.get(norm_ty).clone()
        else {
            return ty;
        };

        let Some(function) = self
            .ctx
            .defs
            .get(def_id.0 as usize)
            .and_then(|def| match def {
                crate::def::Def::Function(function) => Some(function),
                _ => None,
            })
        else {
            return ty;
        };

        if function.generics.is_empty() || generic_args.len() >= function.generics.len() {
            return ty;
        }

        let fn_name = self.ctx.resolve(function.name).to_string();
        self.ctx
            .struct_error(
                expr.span,
                format!(
                    "generic function `{}` cannot be used as a value without explicit instantiation",
                    fn_name
                ),
            )
            .with_hint(format!(
                "use `{}[...]` with concrete generic arguments, for example `{}[i32]`",
                fn_name, fn_name
            ))
            .with_hint("bare generic function items are only allowed in direct call position")
            .emit();
        TypeId::ERROR
    }

    fn reject_resolved_type_namespace_value_expr(
        &mut self,
        span: Span,
        resolved_ty: TypeId,
    ) -> TypeId {
        let resolved_builtin = match self.ctx.type_registry.get(resolved_ty).clone() {
            TypeKind::AnonymousEnum(enum_def) => enum_def.builtin,
            _ => None,
        };

        match resolved_builtin {
            Some(BuiltinAnonymousEnumKind::Optional) => {
                self.ctx
                    .struct_error(
                        span,
                        "optional types cannot be evaluated as value expressions",
                    )
                    .with_hint("optional types are ordinary enum families, not null-pointer syntax")
                    .with_hint("if you meant the empty optional constructor, write `?T.None`")
                    .emit();
            }
            Some(BuiltinAnonymousEnumKind::Result) => {
                self.ctx
                    .struct_error(span, "result types cannot be evaluated as value expressions")
                    .with_hint(
                        "results are types; construct values with `T!E.{ Ok: ... }` or `T!E.{ Err: ... }`",
                    )
                    .emit();
            }
            None => {
                let message = if resolved_ty == TypeId::ERROR {
                    "type expressions cannot be evaluated as values".to_string()
                } else {
                    format!(
                        "type `{}` cannot be evaluated as a value expression",
                        self.ctx.ty_to_string(resolved_ty)
                    )
                };
                self.ctx
                    .struct_error(span, message)
                    .with_hint(
                        "construct a value with `Type.{...}`, access a constructor like `Type.Variant`, or move the type back into a type position",
                    )
                    .emit();
            }
        }
        TypeId::ERROR
    }

    fn reject_type_node_value_expr(&mut self, type_node: &ast::TypeNode) -> TypeId {
        let resolved_ty = self.evaluate_dynamic_typeof(type_node);
        let resolved_builtin = match self.ctx.type_registry.get(resolved_ty).clone() {
            TypeKind::AnonymousEnum(enum_def) => enum_def.builtin,
            _ => None,
        };

        match (&type_node.kind, resolved_builtin) {
            (ast::TypeKind::Optional { .. }, _) | (_, Some(BuiltinAnonymousEnumKind::Optional)) => {
                self.ctx
                    .struct_error(
                        type_node.span,
                        "optional types cannot be evaluated as value expressions",
                    )
                    .with_hint("optional types are ordinary enum families, not null-pointer syntax")
                    .with_hint("if you meant the empty optional constructor, write `?T.None`")
                    .emit();
            }
            (ast::TypeKind::Result { .. }, _) | (_, Some(BuiltinAnonymousEnumKind::Result)) => {
                self.ctx
                    .struct_error(
                        type_node.span,
                        "result types cannot be evaluated as value expressions",
                    )
                    .with_hint(
                        "results are types; construct values with `T!E.{ Ok: ... }` or `T!E.{ Err: ... }`",
                    )
                    .emit();
            }
            _ => {
                return self.reject_resolved_type_namespace_value_expr(type_node.span, resolved_ty);
            }
        }
        TypeId::ERROR
    }

    fn timing_start(&self) -> Option<Instant> {
        self.ctx.collects_timings().then(Instant::now)
    }

    fn record_expr_timing(
        &mut self,
        started: Option<Instant>,
        record: impl FnOnce(&mut crate::context::ExprTimingStats, Duration),
    ) {
        if let Some(started) = started {
            record(&mut self.ctx.analysis.expr_timing_stats, started.elapsed());
        }
    }

    // Pattern/type checking needs fully instantiated field and payload types; dropping const
    // arguments here would let explicit nested pattern types silently drift from the real type.
    pub(crate) fn positional_generic_subst_map(
        &self,
        generics: &[ast::GenericParam],
        generic_args: &[GenericArg],
    ) -> FastHashMap<SymbolId, GenericArg> {
        generics
            .iter()
            .zip(generic_args.iter().copied())
            .map(|(param, arg)| (param.name, arg))
            .collect()
    }

    pub(crate) fn substitute_type_with_generic_arg_map<S: BuildHasher>(
        &mut self,
        ty: TypeId,
        map: &HashMap<SymbolId, GenericArg, S>,
    ) -> TypeId {
        if map.is_empty() {
            return ty;
        }

        let mut subst = crate::checker::Substituter::new(&mut self.ctx.type_registry, map);
        subst.substitute(ty)
    }

    pub(crate) fn generic_param_occurs_in_type_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        ty: TypeId,
        map: &HashMap<SymbolId, TypeId, S>,
    ) -> bool {
        self.generic_param_occurs_in_type_with_map_inner(needle, ty, map, &mut Vec::new())
    }

    fn generic_param_occurs_in_type_with_map_inner<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        ty: TypeId,
        map: &HashMap<SymbolId, TypeId, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        let norm = self.resolve_tv(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Primitive(_)
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_) => false,
            TypeKind::Alias(..) => unreachable!("aliases are removed by resolve_tv"),
            TypeKind::Param(name) => {
                if name == needle {
                    return true;
                }
                if param_stack.contains(&name) {
                    return false;
                }
                let Some(&mapped_ty) = map.get(&name) else {
                    return false;
                };
                param_stack.push(name);
                let occurs = self.generic_param_occurs_in_type_with_map_inner(
                    needle,
                    mapped_ty,
                    map,
                    param_stack,
                );
                param_stack.pop();
                occurs
            }
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::ArrayInfer { elem, .. }
            | TypeKind::AnonymousEnumPayload(elem)
            | TypeKind::Simd { elem, .. } => {
                self.generic_param_occurs_in_type_with_map_inner(needle, elem, map, param_stack)
            }
            TypeKind::Array { elem, len, .. } => {
                self.generic_param_occurs_in_type_with_map_inner(needle, elem, map, param_stack)
                    || self.generic_param_occurs_in_const_generic_with_map(
                        needle,
                        len,
                        map,
                        param_stack,
                    )
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::FnDef(_, args)
            | TypeKind::Associated(_, args) => args.into_iter().any(|arg| {
                self.generic_param_occurs_in_generic_arg_with_map(needle, arg, map, param_stack)
            }),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter().any(|arg| {
                    self.generic_param_occurs_in_generic_arg_with_map(needle, arg, map, param_stack)
                }) || assoc_bindings.into_iter().any(|(_, assoc_ty)| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        assoc_ty,
                        map,
                        param_stack,
                    )
                })
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.generic_param_occurs_in_type_with_map_inner(needle, target, map, param_stack)
                    || trait_args.into_iter().any(|arg| {
                        self.generic_param_occurs_in_generic_arg_with_map(
                            needle,
                            arg,
                            map,
                            param_stack,
                        )
                    })
                    || assoc_args.into_iter().any(|arg| {
                        self.generic_param_occurs_in_generic_arg_with_map(
                            needle,
                            arg,
                            map,
                            param_stack,
                        )
                    })
            }
            TypeKind::ClosureInterface { params, ret } | TypeKind::Function { params, ret, .. } => {
                params.into_iter().any(|param_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        param_ty,
                        map,
                        param_stack,
                    )
                }) || self.generic_param_occurs_in_type_with_map_inner(
                    needle,
                    ret,
                    map,
                    param_stack,
                )
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures.into_iter().any(|capture_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        capture_ty,
                        map,
                        param_stack,
                    )
                }) || params.into_iter().any(|param_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        param_ty,
                        map,
                        param_stack,
                    )
                }) || self.generic_param_occurs_in_type_with_map_inner(
                    needle,
                    ret,
                    map,
                    param_stack,
                )
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => {
                fields.into_iter().any(|field| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        field.ty,
                        map,
                        param_stack,
                    )
                })
            }
            TypeKind::AnonymousEnum(enum_def) => {
                enum_def.backing_ty.is_some_and(|backing_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        backing_ty,
                        map,
                        param_stack,
                    )
                }) || enum_def.variants.iter().any(|variant| {
                    variant.payload_ty.is_some_and(|payload_ty| {
                        self.generic_param_occurs_in_type_with_map_inner(
                            needle,
                            payload_ty,
                            map,
                            param_stack,
                        )
                    })
                })
            }
        }
    }

    fn generic_param_occurs_in_generic_arg_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        arg: GenericArg,
        map: &HashMap<SymbolId, TypeId, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        match arg {
            GenericArg::Type(ty) => {
                self.generic_param_occurs_in_type_with_map_inner(needle, ty, map, param_stack)
            }
            GenericArg::Const(value) => {
                self.generic_param_occurs_in_const_generic_with_map(needle, value, map, param_stack)
            }
        }
    }

    fn generic_param_occurs_in_const_generic_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        value: ConstGeneric,
        map: &HashMap<SymbolId, TypeId, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        match value {
            ConstGeneric::Value(value) => {
                self.generic_param_occurs_in_type_with_map_inner(needle, value.ty, map, param_stack)
            }
            ConstGeneric::Param(_, ty) => {
                self.generic_param_occurs_in_type_with_map_inner(needle, ty, map, param_stack)
            }
            ConstGeneric::Expr(expr_id) => match *self.ctx.type_registry.const_expr(expr_id) {
                ConstExprKind::Unary { expr, ty, .. } | ConstExprKind::Cast { expr, ty } => {
                    self.generic_param_occurs_in_const_generic_with_map(
                        needle,
                        expr,
                        map,
                        param_stack,
                    ) || self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        ty,
                        map,
                        param_stack,
                    )
                }
                ConstExprKind::Binary { lhs, rhs, ty, .. } => {
                    self.generic_param_occurs_in_const_generic_with_map(
                        needle,
                        lhs,
                        map,
                        param_stack,
                    ) || self.generic_param_occurs_in_const_generic_with_map(
                        needle,
                        rhs,
                        map,
                        param_stack,
                    ) || self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        ty,
                        map,
                        param_stack,
                    )
                }
            },
            ConstGeneric::Error => false,
        }
    }

    pub(crate) fn const_param_occurs_in_const_generic_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        value: ConstGeneric,
        map: &HashMap<SymbolId, ConstGeneric, S>,
    ) -> bool {
        self.const_param_occurs_in_const_generic_with_map_inner(needle, value, map, &mut Vec::new())
    }

    fn const_param_occurs_in_const_generic_with_map_inner<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        value: ConstGeneric,
        map: &HashMap<SymbolId, ConstGeneric, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        match value {
            ConstGeneric::Value(_) | ConstGeneric::Error => false,
            ConstGeneric::Param(name, _) => {
                if name == needle {
                    return true;
                }
                if param_stack.contains(&name) {
                    return false;
                }
                let Some(&mapped_value) = map.get(&name) else {
                    return false;
                };
                param_stack.push(name);
                let occurs = self.const_param_occurs_in_const_generic_with_map_inner(
                    needle,
                    mapped_value,
                    map,
                    param_stack,
                );
                param_stack.pop();
                occurs
            }
            ConstGeneric::Expr(expr_id) => match *self.ctx.type_registry.const_expr(expr_id) {
                ConstExprKind::Unary { expr, .. } | ConstExprKind::Cast { expr, .. } => self
                    .const_param_occurs_in_const_generic_with_map_inner(
                        needle,
                        expr,
                        map,
                        param_stack,
                    ),
                ConstExprKind::Binary { lhs, rhs, .. } => {
                    self.const_param_occurs_in_const_generic_with_map_inner(
                        needle,
                        lhs,
                        map,
                        param_stack,
                    ) || self.const_param_occurs_in_const_generic_with_map_inner(
                        needle,
                        rhs,
                        map,
                        param_stack,
                    )
                }
            },
        }
    }

    pub(crate) fn build_generic_subst_map<TS: BuildHasher, CS: BuildHasher>(
        &self,
        type_map: &HashMap<SymbolId, TypeId, TS>,
        const_map: &HashMap<SymbolId, ConstGeneric, CS>,
    ) -> FastHashMap<SymbolId, GenericArg> {
        let mut subst_map = FastHashMap::default();
        for (&name, &ty) in type_map {
            subst_map.insert(name, GenericArg::Type(ty));
        }
        for (&name, &value) in const_map {
            subst_map.insert(name, GenericArg::Const(value));
        }
        subst_map
    }

    pub(crate) fn substitute_type_with_unification_maps<TS: BuildHasher, CS: BuildHasher>(
        &mut self,
        ty: TypeId,
        type_map: &HashMap<SymbolId, TypeId, TS>,
        const_map: &HashMap<SymbolId, ConstGeneric, CS>,
    ) -> TypeId {
        if type_map.is_empty() && const_map.is_empty() {
            return ty;
        }
        let subst_map = self.build_generic_subst_map(type_map, const_map);
        let mut subst = crate::checker::Substituter::new(&mut self.ctx.type_registry, &subst_map);
        subst.substitute(ty)
    }

    fn type_arg_is_direct_const_value_ref(&mut self, ty_node: &kernc_ast::TypeNode) -> bool {
        let ast::TypeKind::Path {
            anchor: None,
            segments,
        } = &ty_node.kind
        else {
            return false;
        };
        let [segment] = segments.as_slice() else {
            return false;
        };
        if !segment.args.is_empty() {
            return false;
        }

        self.ctx
            .scopes
            .resolve_value_symbol(segment.name)
            .is_some_and(|info| {
                matches!(
                    info.kind,
                    crate::scope::SymbolKind::ConstParam | crate::scope::SymbolKind::Const
                )
            })
    }

    fn type_arg_is_payloadless_enum_value_ref(
        &mut self,
        ty_node: &kernc_ast::TypeNode,
        span: Span,
    ) -> bool {
        let ast::TypeKind::Path { anchor, segments } = &ty_node.kind else {
            return false;
        };
        if segments.len() < 2 || segments.iter().any(|segment| !segment.args.is_empty()) {
            return false;
        }

        let Some(last_segment) = segments.last() else {
            return false;
        };
        let mut current_scope = match anchor {
            Some(anchor) => {
                let Some((_, scope)) = self.anchored_start_scope(*anchor, span) else {
                    return false;
                };
                scope
            }
            None => match self.ctx.scopes.current_scope_id() {
                Some(scope) => scope,
                None => return false,
            },
        };

        for (index, segment) in segments[..segments.len() - 1].iter().enumerate() {
            let symbol = if index == 0 && anchor.is_none() {
                self.ctx
                    .scopes
                    .resolve_namespace_from(current_scope, segment.name)
            } else {
                self.ctx
                    .scopes
                    .resolve_namespace_in(current_scope, segment.name)
            };
            let Some(symbol) = symbol.cloned() else {
                return false;
            };

            match symbol.kind {
                crate::scope::SymbolKind::Module => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(crate::def::Def::Module(module)) =
                        self.ctx.defs.get(def_id.0 as usize)
                    else {
                        return false;
                    };
                    current_scope = module.scope_id;
                }
                crate::scope::SymbolKind::Enum if index == segments.len() - 2 => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(crate::def::Def::Enum(enum_def)) =
                        self.ctx.defs.get(def_id.0 as usize)
                    else {
                        return false;
                    };
                    return enum_def.variants.iter().any(|variant| {
                        variant.name == last_segment.name && variant.payload_type.is_none()
                    });
                }
                crate::scope::SymbolKind::TypeAlias if index == segments.len() - 2 => {
                    let alias_ty = self.ctx.type_registry.normalize(symbol.type_id);
                    return match self.ctx.type_registry.get(alias_ty) {
                        TypeKind::Enum(def_id, _) => self
                            .ctx
                            .defs
                            .get(def_id.0 as usize)
                            .and_then(|def| match def {
                                crate::def::Def::Enum(enum_def) => Some(enum_def),
                                _ => None,
                            })
                            .is_some_and(|enum_def| {
                                enum_def.variants.iter().any(|variant| {
                                    variant.name == last_segment.name
                                        && variant.payload_type.is_none()
                                })
                            }),
                        TypeKind::AnonymousEnum(enum_def) => {
                            enum_def.variants.iter().any(|variant| {
                                variant.name == last_segment.name && variant.payload_ty.is_none()
                            })
                        }
                        _ => false,
                    };
                }
                _ => return false,
            }
        }

        false
    }

    /// Main entry point for expression type checking.
    pub(crate) fn check_expr(&mut self, expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        let ty = match &expr.kind {
            ExprKind::Error => TypeId::ERROR,

            // === 1. Primitive literals ===
            ExprKind::Integer(_) => self.check_integer(expr, expected_ty),
            ExprKind::Float(_) => self.check_float(expr, expected_ty),
            ExprKind::Bool(_) => TypeId::BOOL,
            ExprKind::Char(_) => TypeId::U32,
            ExprKind::ByteChar(_) => TypeId::U8,
            ExprKind::String(value) => self.string_literal_type(value),

            // === 2. Identifiers and variables ===
            ExprKind::Identifier(name) => {
                let started = self.timing_start();
                let ty = self.check_identifier(*name, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_identifier += elapsed;
                });
                ty
            }
            ExprKind::AnchoredPath { anchor, name, .. } => {
                let started = self.timing_start();
                let ty = self.check_anchored_identifier(*anchor, *name, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_identifier += elapsed;
                });
                ty
            }
            ExprKind::TypeNode(type_node) => self.reject_type_node_value_expr(type_node),
            ExprKind::SelfValue => self.check_self_value(expr.span),
            ExprKind::Grouped { expr: inner } => self.check_expr(inner, expected_ty),

            // === 3. Declarations and bindings ===
            ExprKind::Let {
                pattern,
                init,
                else_clause,
            } => {
                let started = self.timing_start();
                let ty = self.check_let(
                    expr.id,
                    pattern,
                    init,
                    else_clause.as_ref(),
                    expected_ty,
                    expr.span,
                );
                self.record_expr_timing(started, |stats, elapsed| stats.bindings += elapsed);
                ty
            }
            ExprKind::Static { pattern, init, .. } => {
                let started = self.timing_start();
                let ty = self.check_static(expr.id, pattern, init, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.bindings += elapsed);
                ty
            }

            // === 4. Operators and assignment ===
            ExprKind::Binary { lhs, op, rhs } => {
                let started = self.timing_start();
                let ty = self.check_binary(expr.id, lhs, *op, rhs, expected_ty);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }
            ExprKind::Unary { op, operand } => {
                let started = self.timing_start();
                let ty = self.check_unary(*op, operand, expr.span, expected_ty);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }
            ExprKind::Assign { lhs, op, rhs } => {
                let started = self.timing_start();
                let ty = self.check_assign(lhs, *op, rhs);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }

            // === 5. Casts and coercions ===
            ExprKind::As { lhs, target } => {
                let started = self.timing_start();
                let actual_target_ty = self.evaluate_dynamic_typeof(target);
                let ty = self.check_as_expr(lhs, actual_target_ty);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }
            ExprKind::Propagate { operand, kind } => {
                let started = self.timing_start();
                let ty = self.check_propagate(operand, *kind, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }

            // === 6. Memory access ===
            ExprKind::IndexAccess { lhs, index, is_mut } => {
                let started = self.timing_start();
                let ty = self.check_index_access(lhs, index, *is_mut, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_index += elapsed;
                });
                ty
            }
            ExprKind::FieldAccess {
                lhs,
                field,
                field_span,
            } => {
                let started = self.timing_start();
                let ty = self.check_field_access(expr.id, lhs, *field, *field_span, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_field += elapsed;
                });
                ty
            }
            ExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
                is_mut,
            } => {
                let started = self.timing_start();
                let ty = self.check_slice_op(
                    lhs,
                    start.as_deref(),
                    end.as_deref(),
                    *is_inclusive,
                    *is_mut,
                    expr.span,
                );
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_slice += elapsed;
                });
                ty
            }

            // === 7. Calls and macros ===
            ExprKind::Call { callee, args } => {
                let started = self.timing_start();
                let ty = self.check_call(callee, args, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.call += elapsed;
                    stats.call_plain += elapsed;
                });
                ty
            }
            ExprKind::GenericInstantiation { target, args } => {
                let started = self.timing_start();
                for arg in args {
                    match arg {
                        ast::GenericArg::Type(ty_node)
                        | ast::GenericArg::AssocBinding { value: ty_node, .. } => {
                            if matches!(arg, ast::GenericArg::Type(_))
                                && (self.type_arg_is_direct_const_value_ref(ty_node)
                                    || self
                                        .type_arg_is_payloadless_enum_value_ref(ty_node, expr.span))
                            {
                                continue;
                            }
                            self.evaluate_dynamic_typeof(ty_node);
                        }
                        ast::GenericArg::ConstExpr(expr) => {
                            let _ = expr;
                            // Const generic arguments are resolved by the dedicated type resolver
                            // below. Running ordinary expression checking here misclassifies
                            // const params like `N` as missing runtime identifiers.
                        }
                    }
                }
                let ty = self.check_generic_instantiation(target, args, expr.span);
                if let Some(owner_trait_ty) = self.ctx.method_owner_ty(target.id) {
                    self.ctx.set_method_owner_ty(expr.id, owner_trait_ty);
                }
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.call += elapsed;
                    stats.call_generic_instantiation += elapsed;
                });
                ty
            }
            ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => {
                let started = self.timing_start();
                let ty = self.check_closure(expr.id, captures, params, ret_type, body, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.call += elapsed;
                    stats.call_closure += elapsed;
                });
                ty
            }

            // === 8. Aggregate literals ===
            ExprKind::DataInit { type_node, literal } => {
                let started = self.timing_start();
                let target_ty = if let Some(t_node) = type_node {
                    self.evaluate_dynamic_typeof(t_node)
                } else {
                    self.resolve_data_init_target_type(None, expected_ty, expr.span)
                };
                let ty = self.check_data_init_expr(target_ty, literal, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.aggregate += elapsed);
                ty
            }
            ExprKind::EnumLiteral {
                variant,
                variant_span,
            } => {
                let started = self.timing_start();
                let ty = self.check_enum_literal(*variant, *variant_span, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.aggregate += elapsed);
                ty
            }
            ExprKind::Undef => {
                let started = self.timing_start();
                let ty = self.check_undef(expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.aggregate += elapsed);
                ty
            }

            // === 9. Control flow ===
            ExprKind::Block { stmts, result } => {
                let started = self.timing_start();
                let ty = self.check_block(stmts, result.as_deref(), expected_ty);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_block += elapsed;
                });
                ty
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let started = self.timing_start();
                let ty = self.check_if(cond, then_branch, else_branch.as_deref(), expected_ty);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_if += elapsed;
                });
                ty
            }
            ExprKind::Match { target, arms } => {
                let started = self.timing_start();
                let ty = self.check_match_expr(target, arms, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_match += elapsed;
                });
                ty
            }
            ExprKind::While { cond, body } => {
                let started = self.timing_start();
                let ty = self.check_while(cond, body);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_for += elapsed;
                });
                ty
            }
            ExprKind::Defer { expr: defer_expr } => {
                let started = self.timing_start();
                let ty = self.check_defer(defer_expr);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_defer += elapsed;
                });
                ty
            }
            ExprKind::Break | ExprKind::Continue => TypeId::NEVER,
            ExprKind::Return(val) => {
                let started = self.timing_start();
                self.check_return(val.as_deref(), expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_return += elapsed;
                });
                TypeId::NEVER
            }

            ExprKind::Infer => {
                self.ctx.struct_error(expr.span, "type placeholder `_` cannot be evaluated as an expression")
                    .with_hint("in Kern, `_` is only used as a discard statement (`_ =`), discard binding (`let _ =`), or array length inference (`[_]T`)")
                    .emit();
                TypeId::ERROR
            }
        };

        let norm_ty = self.resolve_tv(ty);
        let ty = if !matches!(expr.kind, ExprKind::TypeNode(_))
            && !matches!(self.ctx.type_registry.get(norm_ty), TypeKind::FnDef(..))
            && self.expr_is_type_namespace(expr)
        {
            self.reject_resolved_type_namespace_value_expr(expr.span, ty)
        } else if self.allow_uninstantiated_generic_function_items {
            ty
        } else {
            self.reject_uninstantiated_generic_function_item(expr, ty)
        };

        let ty = self.maybe_constrain_by_expected_type(ty, expected_ty);
        self.ctx.set_node_type(expr.id, ty);
        self.touched_expr_nodes.push(expr.id);
        ty
    }

    /// Recursively scan AST type nodes, resolve every `@typeOf`, and rebuild the final type bottom-up.
    pub(crate) fn evaluate_dynamic_typeof(&mut self, ty_node: &kernc_ast::TypeNode) -> TypeId {
        let started = self.timing_start();
        let ty_id = match &ty_node.kind {
            ast::TypeKind::Error => TypeId::ERROR,
            ast::TypeKind::TypeOf(inner_expr) => self.check_expr(inner_expr, None),
            ast::TypeKind::Optional { inner } => {
                let inner_ty = self.evaluate_dynamic_typeof(inner);
                let some = self.ctx.intern("Some");
                let none = self.ctx.intern("None");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: None,
                        builtin: Some(BuiltinAnonymousEnumKind::Optional),
                        variants: vec![
                            crate::ty::AnonymousVariant {
                                name: some,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: Some(inner_ty),
                                explicit_value: None,
                            },
                            crate::ty::AnonymousVariant {
                                name: none,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: None,
                                explicit_value: None,
                            },
                        ],
                    }))
            }
            ast::TypeKind::Result { ok, err } => {
                let ok_ty = self.evaluate_dynamic_typeof(ok);
                let err_ty = self.evaluate_dynamic_typeof(err);
                let ok_name = self.ctx.intern("Ok");
                let err_name = self.ctx.intern("Err");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: None,
                        builtin: Some(BuiltinAnonymousEnumKind::Result),
                        variants: vec![
                            crate::ty::AnonymousVariant {
                                name: ok_name,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: Some(ok_ty),
                                explicit_value: None,
                            },
                            crate::ty::AnonymousVariant {
                                name: err_name,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: Some(err_ty),
                                explicit_value: None,
                            },
                        ],
                    }))
            }
            ast::TypeKind::Pointer { is_mut, elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::VolatilePtr { is_mut, elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Slice { is_mut, elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::ArrayInfer { elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx
                    .type_registry
                    .intern(TypeKind::ArrayInfer { elem: base })
            }
            ast::TypeKind::Array { elem, len } => {
                let base = self.evaluate_dynamic_typeof(elem);
                let references_const_param = {
                    let mut resolver = TypeResolver::new(self.ctx);
                    let Some(scope) = resolver.current_scope_id() else {
                        return TypeId::ERROR;
                    };
                    resolver.expr_references_const_param(len, scope)
                };

                let resolved_len = if references_const_param {
                    let mut resolver = TypeResolver::new(self.ctx);
                    let Some(scope) = resolver.current_scope_id() else {
                        return TypeId::ERROR;
                    };
                    resolver.resolve_const_generic_expr(len, TypeId::USIZE, scope, "array length")
                } else {
                    let Ok(length) = crate::checker::ConstEvaluator::new(self.ctx).eval_usize(len)
                    else {
                        return TypeId::ERROR;
                    };
                    crate::ty::ConstGeneric::Value(crate::ty::ConstGenericValue {
                        ty: TypeId::USIZE,
                        kind: crate::ty::ConstGenericValueKind::Int(length as i128),
                    })
                };

                if matches!(resolved_len, crate::ty::ConstGeneric::Error) {
                    return TypeId::ERROR;
                }
                if let crate::ty::ConstGeneric::Value(value) = resolved_len
                    && let Some(length) = value.as_int()
                    && length > u32::MAX as i128
                {
                    self.ctx
                        .struct_error(
                            len.span,
                            format!(
                                "array length {} exceeds the current compiler limit of {} elements",
                                length,
                                u32::MAX
                            ),
                        )
                        .with_hint(
                            "LLVM array types are emitted with a 32-bit element count; split the object or allocate dynamically instead",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                self.ctx.type_registry.intern(TypeKind::Array {
                    elem: base,
                    len: resolved_len,
                })
            }
            ast::TypeKind::ClosureInterface { params, ret } => {
                let mut param_tys = Vec::new();
                for p in params {
                    param_tys.push(self.evaluate_dynamic_typeof(p));
                }
                let ret_ty = if let Some(r) = ret {
                    self.evaluate_dynamic_typeof(r)
                } else {
                    TypeId::VOID
                };
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: param_tys,
                    ret: ret_ty,
                })
            }
            // Plain static types such as `Path` or `SelfType` cannot contain nested `@typeOf`.
            // Delegate them directly to the type resolver.
            _ => {
                let mut resolver = TypeResolver::new(self.ctx);
                let Some(scope) = resolver.current_scope_id() else {
                    self.ctx.emit_ice(
                        ty_node.span,
                        "Kern ICE (Typeck): missing current scope while resolving `@typeOf`.",
                    );
                    return TypeId::ERROR;
                };
                resolver.resolve_type(ty_node, scope)
            }
        };

        // Overwrite the cached node type with the freshly resolved result.
        self.ctx.set_node_type(ty_node.id, ty_id);
        self.record_expr_timing(started, |stats, elapsed| stats.dynamic_typeof += elapsed);
        ty_id
    }

    fn check_propagate(
        &mut self,
        operand: &Expr,
        kind: ast::PropagateKind,
        span: kernc_utils::Span,
    ) -> TypeId {
        let Some(current_return_ty) = self.current_return_type else {
            self.ctx
                .struct_error(
                    span,
                    "propagation is only valid inside functions with a return type",
                )
                .emit();
            return TypeId::ERROR;
        };
        let norm_return = self.resolve_tv(current_return_ty);

        let TypeKind::AnonymousEnum(return_enum) = self.ctx.type_registry.get(norm_return).clone()
        else {
            let ret_str = self.ctx.ty_to_string(current_return_ty);
            self.ctx
                .struct_error(
                    span,
                    format!("propagation target function must return a builtin optional/result, found `{}`", ret_str),
                )
                .emit();
            return TypeId::ERROR;
        };

        let operand_expected = match kind {
            ast::PropagateKind::Option => Some(current_return_ty),
            ast::PropagateKind::Result => {
                let Some((_, ret_err_ty)) = return_enum.builtin_result_types() else {
                    let ret_str = self.ctx.ty_to_string(current_return_ty);
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`.!` requires the enclosing function to return a builtin result, found `{}`",
                                ret_str
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                };

                let ok = self.fresh_type_var();
                let ok_name = self.ctx.intern("Ok");
                let err_name = self.ctx.intern("Err");
                Some(
                    self.ctx
                        .type_registry
                        .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                            backing_ty: None,
                            builtin: Some(BuiltinAnonymousEnumKind::Result),
                            variants: vec![
                                AnonymousVariant {
                                    name: ok_name,
                                    name_span: Span::default(),
                                    payload_ty: Some(ok),
                                    explicit_value: None,
                                },
                                AnonymousVariant {
                                    name: err_name,
                                    name_span: Span::default(),
                                    payload_ty: Some(ret_err_ty),
                                    explicit_value: None,
                                },
                            ],
                        })),
                )
            }
        };

        let operand_ty = self.check_expr(operand, operand_expected);
        let norm_operand = self.resolve_tv(operand_ty);

        let TypeKind::AnonymousEnum(operand_enum) =
            self.ctx.type_registry.get(norm_operand).clone()
        else {
            let op = match kind {
                ast::PropagateKind::Option => ".?",
                ast::PropagateKind::Result => ".!",
            };
            let found = self.ctx.ty_to_string(operand_ty);
            self.ctx
                .struct_error(
                    span,
                    format!("`{}` requires a builtin optional or result value", op),
                )
                .with_hint(format!("found `{}`", found))
                .emit();
            return TypeId::ERROR;
        };

        match kind {
            ast::PropagateKind::Option => {
                let Some(inner_ty) = operand_enum.builtin_optional_payload() else {
                    self.ctx
                        .struct_error(span, "`.?` requires a builtin optional value")
                        .emit();
                    return TypeId::ERROR;
                };
                if return_enum.builtin != Some(BuiltinAnonymousEnumKind::Optional) {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`.?` requires the enclosing function to return a builtin optional, found `{}`",
                                self.ctx.ty_to_string(current_return_ty)
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                inner_ty
            }
            ast::PropagateKind::Result => {
                let Some((ok_ty, err_ty)) = operand_enum.builtin_result_types() else {
                    self.ctx
                        .struct_error(span, "`.!` requires a builtin result value")
                        .emit();
                    return TypeId::ERROR;
                };
                let Some((_, ret_err_ty)) = return_enum.builtin_result_types() else {
                    let ret_str = self.ctx.ty_to_string(current_return_ty);
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`.!` requires the enclosing function to return a builtin result, found `{}`",
                                ret_str
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                };
                if err_ty != ret_err_ty && err_ty != TypeId::ERROR && ret_err_ty != TypeId::ERROR {
                    self.emit_mismatch_error(span, err_ty, ret_err_ty);
                    return TypeId::ERROR;
                }
                ok_ty
            }
        }
    }
}
