use super::ImportResolver;
use crate::SemaContext;
use crate::checker::{ConstEvaluator, ConstValue, ExprChecker, Substituter};
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{
    AnonymousEnum, AnonymousField, AnonymousVariant, BuiltinAnonymousEnumKind, ConstExprBinaryOp,
    ConstExprKind, ConstExprUnaryOp, ConstGeneric, ConstGenericValue, ConstGenericValueKind,
    GenericArg, LayoutEngine, PrimitiveType, TypeId, TypeKind,
};
use kernc_ast::{self as ast, BinaryOperator, UnaryOperator, Visibility};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

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
}

struct PendingTraitProjection {
    trait_def_id: DefId,
    trait_args: Vec<GenericArg>,
    assoc_bindings: Vec<(DefId, TypeId)>,
}

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            suppress_unqualified_impl_assoc_types: false,
        }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn current_scope_id(&self) -> Option<ScopeId> {
        self.ctx.scopes.current_scope_id()
    }
}
