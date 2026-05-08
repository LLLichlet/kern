use super::ImportResolver;
use crate::SemaContext;
use crate::checker::{ConstEvaluator, ConstValue, ExprChecker};
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{
    AnonymousEnum, AnonymousField, AnonymousVariant, BuiltinAnonymousEnumKind, ConstExprBinaryOp,
    ConstExprKind, ConstExprUnaryOp, ConstGeneric, ConstGenericValue, ConstGenericValueKind,
    GenericArg, LayoutEngine, PrimitiveType, Substituter, TypeId, TypeKind,
};
use kernc_ast::{self as ast, BinaryOperator, UnaryOperator, Visibility};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;
use std::time::{Duration, Instant};

mod assoc_types;
mod const_generics;
mod contract_subst;
mod generic_bounds;
mod helper;
mod impl_validation;
mod items;
mod resolve_expr;
mod resolve_path;
mod resolve_type;
mod supertraits;

pub struct TypeResolver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    suppress_unqualified_impl_assoc_types: bool,
    phase_timings: TypeResolutionPhaseTimings,
}

struct PendingTraitProjection {
    trait_def_id: DefId,
    trait_args: Vec<GenericArg>,
    assoc_bindings: Vec<(DefId, TypeId)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeResolutionTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Debug, Default, Clone, Copy)]
struct TypeResolutionPhaseTimings {
    resolve_alias_items: Duration,
    resolve_non_alias_items: Duration,
    validate_supertrait_graph: Duration,
    validate_trait_impl_coherence: Duration,
    validate_impl_associated_type_targets: Duration,
}

impl TypeResolutionPhaseTimings {
    fn phase_timings(self) -> Vec<TypeResolutionTiming> {
        [
            ("    resolve_alias_items", self.resolve_alias_items),
            ("    resolve_non_alias_items", self.resolve_non_alias_items),
            (
                "    validate_supertrait_graph",
                self.validate_supertrait_graph,
            ),
            (
                "    validate_trait_impl_coherence",
                self.validate_trait_impl_coherence,
            ),
            (
                "    validate_impl_associated_type_targets",
                self.validate_impl_associated_type_targets,
            ),
        ]
        .into_iter()
        .filter_map(|(name, duration)| {
            if duration == Duration::default() {
                None
            } else {
                Some(TypeResolutionTiming { name, duration })
            }
        })
        .collect()
    }
}

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            suppress_unqualified_impl_assoc_types: false,
            phase_timings: TypeResolutionPhaseTimings::default(),
        }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn phase_timings(&self) -> Vec<TypeResolutionTiming> {
        if !self.ctx.collects_timings() {
            return Vec::new();
        }
        self.phase_timings.phase_timings()
    }

    pub fn current_scope_id(&self) -> Option<ScopeId> {
        self.ctx.scopes.current_scope_id()
    }

    fn measure_phase<T, F, R>(&mut self, record: R, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
        R: FnOnce(&mut TypeResolutionPhaseTimings, Duration),
    {
        if !self.ctx.collects_timings() {
            return f(self);
        }

        let started = Instant::now();
        let value = f(self);
        record(&mut self.phase_timings, started.elapsed());
        value
    }
}
