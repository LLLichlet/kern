use crate::LayoutEngine;
use crate::SemaContext;
use crate::def::{Def, DefId, EnumDef, FunctionDef, GlobalDef, ModuleDef, StructDef, UnionDef};
use crate::scope::ScopeId;
use crate::scope::SymbolKind;
use crate::ty::{GenericArg, PrimitiveType, Substituter, TypeId, TypeKind};
pub use core::{
    ConstArithmeticError, ConstBinaryOp, ConstEvalCore, ConstEvalError, ConstEvalResult,
    ConstFunctionFrame, ConstPlace, ConstPlaceError, ConstValue, LoopControl, PlaceSegment,
    ScriptHost, ScriptHostHandle,
};
use kernc_ast::{self as ast, AssignmentOperator, BinaryOperator, Expr, ExprKind, UnaryOperator};
use kernc_utils::{NodeId, Span, SymbolId};
use std::collections::HashMap;

mod call;
mod core;
mod data;
mod eval;
mod place;
mod state;

#[derive(Debug, Clone, Copy)]
struct DefScopeFrame {
    prev_scope: Option<ScopeId>,
    owner_scope: Option<ScopeId>,
}

pub struct ConstEvaluator<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    const_scopes: Vec<ScopeId>,
    core: ConstEvalCore,
}

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        let mut const_scopes = Vec::new();
        if let Some(scope_id) = ctx.scopes.current_scope_id() {
            const_scopes.push(scope_id);
        }

        Self {
            ctx,
            const_scopes,
            core: ConstEvalCore::default(),
        }
    }

    pub fn with_script_host<H: ScriptHost>(ctx: &'a mut SemaContext<'ctx>, host: &mut H) -> Self {
        let mut this = Self::new(ctx);
        this.core.set_script_host(ScriptHostHandle::new(host));
        this.core.set_allow_non_const_calls(true);
        this
    }

    pub fn with_type_substs(mut self, subst_map: &HashMap<SymbolId, GenericArg>) -> Self {
        self.core.push_type_subst(subst_map.clone());
        self
    }

    /// Evaluate a constant expression that must yield a non-negative usize-like value.
    pub fn eval_usize(&mut self, expr: &Expr) -> ConstEvalResult<u64> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => {
                if val < 0 {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "constant expression cannot evaluate to a negative number here",
                        )
                        .with_hint("array lengths and similar contexts require positive integers")
                        .emit();
                    Err(ConstEvalError)
                } else {
                    match u64::try_from(val) {
                        Ok(value) => Ok(value),
                        Err(_) => {
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    "constant expression is too large for this usize-like context",
                                )
                                .with_hint(format!(
                                    "array lengths and similar contexts require values in the range 0 to {}",
                                    u64::MAX
                                ))
                                .emit();
                            Err(ConstEvalError)
                        }
                    }
                }
            }
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(ConstEvalError)
            }
            Err(_) => Err(ConstEvalError),
        }
    }

    /// Evaluate a constant integer expression without applying usize restrictions.
    pub fn eval_math(&mut self, expr: &Expr) -> ConstEvalResult<i128> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => Ok(val),
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(ConstEvalError)
            }
            Err(_) => Err(ConstEvalError),
        }
    }

    pub fn eval_const_value(&mut self, expr: &Expr) -> ConstEvalResult<ConstValue> {
        self.eval_inner(expr, 0)
    }

    pub(super) fn layout_size(&mut self, ty: TypeId) -> ConstEvalResult<u64> {
        let errors_before = self.ctx.sess.error_count;
        let size = {
            let mut layout = LayoutEngine::new(self.ctx);
            layout.compute_type_size(ty)
        };
        if self.ctx.sess.error_count != errors_before {
            Err(ConstEvalError)
        } else {
            Ok(size)
        }
    }

    pub(super) fn layout_align(&mut self, ty: TypeId) -> ConstEvalResult<u64> {
        let errors_before = self.ctx.sess.error_count;
        let align = {
            let mut layout = LayoutEngine::new(self.ctx);
            layout.compute_type_align(ty)
        };
        if self.ctx.sess.error_count != errors_before {
            Err(ConstEvalError)
        } else {
            Ok(align)
        }
    }

    fn enter_def_scope(&mut self, def_id: DefId) -> DefScopeFrame {
        let owner_scope = self.def_owner_scope(def_id);
        let prev_scope = self.ctx.scopes.current_scope_id();
        if let Some(owner_scope) = owner_scope {
            self.ctx.scopes.set_current_scope(owner_scope);
            self.const_scopes.push(owner_scope);
        }

        DefScopeFrame {
            prev_scope,
            owner_scope,
        }
    }

    fn leave_def_scope(&mut self, frame: DefScopeFrame) {
        if frame.owner_scope.is_some() {
            let _ = self.const_scopes.pop();
        }
        if let Some(prev_scope) = frame.prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }
    }

    fn enter_function_frame(
        &mut self,
        return_ty: TypeId,
        has_generic_substs: bool,
    ) -> ConstFunctionFrame {
        self.core
            .enter_function_frame(return_ty, has_generic_substs)
    }

    fn leave_function_frame(&mut self, frame: ConstFunctionFrame) -> Option<ConstValue> {
        self.core.leave_function_frame(frame)
    }

    fn current_expected_type(&self) -> Option<TypeId> {
        self.core.current_expected_type()
    }

    fn with_local_scope<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> ConstEvalResult<T>,
    ) -> ConstEvalResult<T> {
        self.push_local_scope();
        let result = f(self);
        self.pop_local_scope();
        result
    }

    fn source_location_value(&mut self, span: Span) -> ConstValue {
        let file_name = self.ctx.intern("file");
        let line_name = self.ctx.intern("line");
        let col_name = self.ctx.intern("col");
        let file = self
            .ctx
            .sess
            .source_manager
            .get_file_path(span.file)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let (line, col) = self
            .ctx
            .sess
            .source_manager
            .lookup_location(span)
            .map(|loc| (loc.line, loc.col))
            .unwrap_or((0, 0));

        let mut fields = HashMap::new();
        fields.insert(file_name, ConstValue::String(file));
        fields.insert(line_name, ConstValue::Int(line as i128));
        fields.insert(col_name, ConstValue::Int(col as i128));
        ConstValue::Struct(fields)
    }

    fn function_def(&self, def_id: DefId) -> Option<FunctionDef> {
        match self.ctx.defs.get(def_id.0 as usize)? {
            Def::Function(func) => Some(func.clone()),
            _ => None,
        }
    }

    fn global_def(&self, def_id: DefId) -> Option<GlobalDef> {
        match self.ctx.defs.get(def_id.0 as usize)? {
            Def::Global(global) => Some(global.clone()),
            _ => None,
        }
    }

    fn module_def(&self, def_id: DefId) -> Option<&ModuleDef> {
        match self.ctx.defs.get(def_id.0 as usize)? {
            Def::Module(module) => Some(module),
            _ => None,
        }
    }

    fn enum_def(&self, def_id: DefId) -> Option<EnumDef> {
        match self.ctx.defs.get(def_id.0 as usize)? {
            Def::Enum(enum_def) => Some(enum_def.clone()),
            _ => None,
        }
    }

    fn struct_or_union_def(&self, def_id: DefId) -> Option<ConstDataDef> {
        match self.ctx.defs.get(def_id.0 as usize)? {
            Def::Struct(def) => Some(ConstDataDef::Struct(def.clone())),
            Def::Union(def) => Some(ConstDataDef::Union(def.clone())),
            _ => None,
        }
    }

    fn resolve_symbol(&self, symbol: SymbolId) -> String {
        self.ctx.resolve(symbol).to_string()
    }

    fn ty_to_string(&self, ty: TypeId) -> String {
        self.ctx.ty_to_string(ty)
    }

    fn normalize_type(&self, ty: TypeId) -> TypeId {
        self.ctx.type_registry.normalize(ty)
    }

    fn type_kind(&self, ty: TypeId) -> TypeKind {
        self.ctx.type_registry.get(ty).clone()
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable (`let`)",
            SymbolKind::Const => "constant",
            SymbolKind::ConstParam => "const parameter",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "data type",
            _ => "symbol",
        }
    }
}

#[derive(Debug, Clone)]
enum ConstDataDef {
    Struct(StructDef),
    Union(UnionDef),
}
