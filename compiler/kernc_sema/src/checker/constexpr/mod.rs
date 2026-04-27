use crate::LayoutEngine;
use crate::SemaContext;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::scope::ScopeId;
use crate::scope::SymbolKind;
use crate::ty::{GenericArg, PrimitiveType, TypeId, TypeKind};
use kernc_ast::{
    self as ast, AssignmentOperator, BinaryOperator, Expr, ExprKind, StmtKind, UnaryOperator,
};
use kernc_utils::{NodeId, Span, SymbolId};
use std::collections::HashMap;

mod call;
mod data;
mod eval;
mod place;
mod state;

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    String(String),
    Array(Vec<ConstValue>),
    Struct(HashMap<SymbolId, ConstValue>),
    Enum {
        tag: i128,
        payload: Option<Box<ConstValue>>,
    },
    Pointer {
        root_scope: usize,
        root_name: SymbolId,
        path: Vec<PlaceSegment>,
        is_mut: bool,
    },
    Void,
    Undef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstEvalError;

type ConstEvalResult<T> = Result<T, ConstEvalError>;

pub trait ScriptHost {
    fn call_extern(
        &mut self,
        name: &str,
        args: &[ConstValue],
        span: Span,
    ) -> Result<ConstValue, String>;
}

#[derive(Clone, Copy)]
struct ScriptHostHandle {
    data: *mut (),
    call_extern: unsafe fn(*mut (), &str, &[ConstValue], Span) -> Result<ConstValue, String>,
}

unsafe fn call_script_host<H: ScriptHost>(
    data: *mut (),
    name: &str,
    args: &[ConstValue],
    span: Span,
) -> Result<ConstValue, String> {
    unsafe { (&mut *(data as *mut H)).call_extern(name, args, span) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Break,
    Continue,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceSegment {
    Field(SymbolId),
    Index(usize),
}

#[derive(Debug, Clone)]
struct ResolvedPlace {
    root_scope: usize,
    root_name: SymbolId,
    path: Vec<PlaceSegment>,
    require_root_mutability: bool,
}

pub struct ConstEvaluator<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    const_scopes: Vec<ScopeId>,
    local_scopes: Vec<HashMap<SymbolId, ConstValue>>,
    local_type_scopes: Vec<HashMap<SymbolId, TypeId>>,
    local_mut_scopes: Vec<HashMap<SymbolId, bool>>,
    type_substs: Vec<HashMap<SymbolId, GenericArg>>,
    expected_types: Vec<TypeId>,
    function_return_types: Vec<TypeId>,
    return_value: Option<ConstValue>,
    function_depth: usize,
    loop_depth: usize,
    loop_control: Option<LoopControl>,
    script_host: Option<ScriptHostHandle>,
    allow_non_const_calls: bool,
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
            local_scopes: Vec::new(),
            local_type_scopes: Vec::new(),
            local_mut_scopes: Vec::new(),
            type_substs: Vec::new(),
            expected_types: Vec::new(),
            function_return_types: Vec::new(),
            return_value: None,
            function_depth: 0,
            loop_depth: 0,
            loop_control: None,
            script_host: None,
            allow_non_const_calls: false,
        }
    }

    pub fn with_script_host<H: ScriptHost>(ctx: &'a mut SemaContext<'ctx>, host: &mut H) -> Self {
        let mut this = Self::new(ctx);
        this.script_host = Some(ScriptHostHandle {
            data: host as *mut H as *mut (),
            call_extern: call_script_host::<H>,
        });
        this.allow_non_const_calls = true;
        this
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
