#![doc = include_str!("../README.md")]

use kernc_ty::{GenericArg, TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

/// Fully materialized value produced by compile-time evaluation.
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

impl ConstValue {
    pub fn project(&self, path: &[PlaceSegment]) -> Result<ConstValue, ConstPlaceError> {
        project_const_value(self, path)
    }

    pub fn project_mut(
        &mut self,
        path: &[PlaceSegment],
    ) -> Result<&mut ConstValue, ConstPlaceError> {
        project_const_value_mut(self, path)
    }

    pub fn pointer_place(&self, require_mut: bool) -> Result<ConstPlace, ConstPlaceError> {
        match self {
            ConstValue::Pointer {
                root_scope,
                root_name,
                path,
                is_mut,
            } => {
                if require_mut && !*is_mut {
                    return Err(ConstPlaceError::ImmutablePointer);
                }

                Ok(ConstPlace {
                    root_scope: *root_scope,
                    root_name: *root_name,
                    path: path.clone(),
                    require_root_mutability: false,
                })
            }
            _ => Err(ConstPlaceError::ExpectedPointer),
        }
    }
}

/// Segment in a compile-time place projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceSegment {
    Field(SymbolId),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstPlace {
    pub root_scope: usize,
    pub root_name: SymbolId,
    pub path: Vec<PlaceSegment>,
    pub require_root_mutability: bool,
}

#[derive(Debug, Default, Clone)]
pub struct ConstLocalStore {
    value_scopes: Vec<HashMap<SymbolId, ConstValue>>,
    mutability_scopes: Vec<HashMap<SymbolId, bool>>,
}

impl ConstLocalStore {
    pub fn push_scope(&mut self) {
        self.value_scopes.push(HashMap::new());
        self.mutability_scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        let _ = self.value_scopes.pop();
        let _ = self.mutability_scopes.pop();
    }

    pub fn define(&mut self, name: SymbolId, value: ConstValue) {
        self.ensure_scope();
        if let Some(scope) = self.value_scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    pub fn define_mutability(&mut self, name: SymbolId, is_mut: bool) {
        self.ensure_scope();
        if let Some(scope) = self.mutability_scopes.last_mut() {
            scope.insert(name, is_mut);
        }
    }

    pub fn lookup(&self, name: SymbolId) -> Option<ConstValue> {
        self.value_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).cloned())
    }

    pub fn lookup_slot(&self, name: SymbolId) -> Option<usize> {
        self.value_scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(scope_idx, scope)| scope.contains_key(&name).then_some(scope_idx))
    }

    pub fn lookup_at(&self, scope_idx: usize, name: SymbolId) -> Option<ConstValue> {
        self.value_scopes
            .get(scope_idx)
            .and_then(|scope| scope.get(&name).cloned())
    }

    pub fn lookup_mutability_at(&self, scope_idx: usize, name: SymbolId) -> Option<bool> {
        self.mutability_scopes
            .get(scope_idx)
            .and_then(|scope| scope.get(&name).copied())
    }

    pub fn assign_at(&mut self, scope_idx: usize, name: SymbolId, value: ConstValue) -> bool {
        if let Some(scope) = self.value_scopes.get_mut(scope_idx)
            && let Some(slot) = scope.get_mut(&name)
        {
            *slot = value;
            return true;
        }
        false
    }

    fn ensure_scope(&mut self) {
        if self.value_scopes.is_empty() {
            self.push_scope();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopControl {
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub struct ConstFunctionFrame {
    saved_loop_depth: usize,
    saved_loop_control: Option<LoopControl>,
    saved_return: Option<ConstValue>,
    has_generic_substs: bool,
}

impl ConstFunctionFrame {
    pub fn has_generic_substs(&self) -> bool {
        self.has_generic_substs
    }
}

#[derive(Debug, Clone)]
pub struct ConstLocalTypes<T> {
    scopes: Vec<HashMap<SymbolId, T>>,
}

impl<T> Default for ConstLocalTypes<T> {
    fn default() -> Self {
        Self { scopes: Vec::new() }
    }
}

impl<T: Copy> ConstLocalTypes<T> {
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        let _ = self.scopes.pop();
    }

    pub fn define(&mut self, name: SymbolId, ty: T) {
        if self.scopes.is_empty() {
            self.push_scope();
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, ty);
        }
    }

    pub fn lookup(&self, name: SymbolId) -> Option<T> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }
}

#[derive(Debug, Clone)]
pub struct ConstExecState<T> {
    expected_types: Vec<T>,
    function_return_types: Vec<T>,
    return_value: Option<ConstValue>,
    function_depth: usize,
    loop_depth: usize,
    loop_control: Option<LoopControl>,
}

#[derive(Clone)]
pub struct ConstEvalCore {
    locals: ConstLocalStore,
    local_types: ConstLocalTypes<TypeId>,
    exec: ConstExecState<TypeId>,
    type_substs: Vec<HashMap<SymbolId, GenericArg>>,
    script_host: Option<ScriptHostHandle>,
    allow_non_const_calls: bool,
}

impl Default for ConstEvalCore {
    fn default() -> Self {
        Self {
            locals: ConstLocalStore::default(),
            local_types: ConstLocalTypes::default(),
            exec: ConstExecState::default(),
            type_substs: Vec::new(),
            script_host: None,
            allow_non_const_calls: false,
        }
    }
}

impl ConstEvalCore {
    pub fn set_script_host(&mut self, host: ScriptHostHandle) {
        self.script_host = Some(host);
    }

    pub fn script_host(&self) -> Option<ScriptHostHandle> {
        self.script_host
    }

    pub fn set_allow_non_const_calls(&mut self, allow: bool) {
        self.allow_non_const_calls = allow;
    }

    pub fn allow_non_const_calls(&self) -> bool {
        self.allow_non_const_calls
    }

    pub fn push_type_subst(&mut self, subst: HashMap<SymbolId, GenericArg>) {
        if !subst.is_empty() {
            self.type_substs.push(subst);
        }
    }

    pub fn pop_type_subst(&mut self) {
        let _ = self.type_substs.pop();
    }

    pub fn type_substs(&self) -> &[HashMap<SymbolId, GenericArg>] {
        &self.type_substs
    }

    pub fn push_local_scope(&mut self) {
        self.locals.push_scope();
        self.local_types.push_scope();
    }

    pub fn pop_local_scope(&mut self) {
        self.locals.pop_scope();
        self.local_types.pop_scope();
    }

    pub fn define_local(&mut self, name: SymbolId, value: ConstValue) {
        self.locals.define(name, value);
    }

    pub fn define_local_type(&mut self, name: SymbolId, ty: TypeId) {
        self.local_types.define(name, ty);
    }

    pub fn define_local_mutability(&mut self, name: SymbolId, is_mut: bool) {
        self.locals.define_mutability(name, is_mut);
    }

    pub fn lookup_local(&self, name: SymbolId) -> Option<ConstValue> {
        self.locals.lookup(name)
    }

    pub fn lookup_local_slot(&self, name: SymbolId) -> Option<usize> {
        self.locals.lookup_slot(name)
    }

    pub fn lookup_local_at(&self, scope_idx: usize, name: SymbolId) -> Option<ConstValue> {
        self.locals.lookup_at(scope_idx, name)
    }

    pub fn lookup_local_mutability_at(&self, scope_idx: usize, name: SymbolId) -> Option<bool> {
        self.locals.lookup_mutability_at(scope_idx, name)
    }

    pub fn lookup_local_type(&self, name: SymbolId) -> Option<TypeId> {
        self.local_types.lookup(name)
    }

    pub fn assign_local_at(
        &mut self,
        scope_idx: usize,
        name: SymbolId,
        value: ConstValue,
    ) -> bool {
        self.locals.assign_at(scope_idx, name, value)
    }

    pub fn enter_function_frame(
        &mut self,
        return_ty: TypeId,
        has_generic_substs: bool,
    ) -> ConstFunctionFrame {
        self.exec.enter_function(return_ty, has_generic_substs)
    }

    pub fn leave_function_frame(&mut self, frame: ConstFunctionFrame) -> Option<ConstValue> {
        let has_generic_substs = frame.has_generic_substs();
        let value = self.exec.leave_function(frame);
        if has_generic_substs {
            self.pop_type_subst();
        }
        value
    }

    pub fn enter_loop(&mut self) {
        self.exec.enter_loop();
    }

    pub fn leave_loop(&mut self) {
        self.exec.leave_loop();
    }

    pub fn in_function(&self) -> bool {
        self.exec.in_function()
    }

    pub fn in_loop(&self) -> bool {
        self.exec.in_loop()
    }

    pub fn current_function_depth(&self) -> usize {
        self.exec.current_function_depth()
    }

    pub fn has_return_value(&self) -> bool {
        self.exec.has_return_value()
    }

    pub fn set_return_value(&mut self, value: ConstValue) {
        self.exec.set_return_value(value);
    }

    pub fn current_return_type(&self) -> Option<TypeId> {
        self.exec.current_return_type()
    }

    pub fn push_expected_type(&mut self, ty: TypeId) {
        self.exec.push_expected_type(ty);
    }

    pub fn pop_expected_type(&mut self) {
        self.exec.pop_expected_type();
    }

    pub fn current_expected_type(&self) -> Option<TypeId> {
        self.exec.current_expected_type()
    }

    pub fn loop_control(&self) -> Option<LoopControl> {
        self.exec.loop_control()
    }

    pub fn take_loop_control(&mut self) -> Option<LoopControl> {
        self.exec.take_loop_control()
    }

    pub fn set_loop_control(&mut self, control: LoopControl) {
        self.exec.set_loop_control(control);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstArithmeticError {
    DivisionByZero,
    ModuloByZero,
    DivisionOverflow,
    ModuloOverflow,
    NegativeShift,
    ShiftTooLarge,
    UnsupportedIntegerOperator,
    UnsupportedFloatOperator,
    UnsupportedBoolOperator,
    UnsupportedEnumOperator,
    TypeMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstBinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    ShiftLeft,
    ShiftRight,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    LogicalAnd,
    LogicalOr,
    Equal,
    NotEqual,
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
}

pub fn eval_const_int_division(lhs: i128, rhs: i128) -> Result<i128, ConstArithmeticError> {
    if rhs == 0 {
        return Err(ConstArithmeticError::DivisionByZero);
    }
    lhs.checked_div(rhs)
        .ok_or(ConstArithmeticError::DivisionOverflow)
}

pub fn eval_const_int_modulo(lhs: i128, rhs: i128) -> Result<i128, ConstArithmeticError> {
    if rhs == 0 {
        return Err(ConstArithmeticError::ModuloByZero);
    }
    lhs.checked_rem(rhs)
        .ok_or(ConstArithmeticError::ModuloOverflow)
}

pub fn eval_const_uint_division(lhs: i128, rhs: i128) -> Result<i128, ConstArithmeticError> {
    if rhs == 0 {
        return Err(ConstArithmeticError::DivisionByZero);
    }
    Ok(((lhs as u128) / (rhs as u128)) as i128)
}

pub fn eval_const_uint_modulo(lhs: i128, rhs: i128) -> Result<i128, ConstArithmeticError> {
    if rhs == 0 {
        return Err(ConstArithmeticError::ModuloByZero);
    }
    Ok(((lhs as u128) % (rhs as u128)) as i128)
}

pub fn eval_const_int_shift(
    lhs: i128,
    rhs: i128,
    is_left: bool,
    unsigned_lhs: bool,
) -> Result<i128, ConstArithmeticError> {
    if rhs < 0 {
        return Err(ConstArithmeticError::NegativeShift);
    }

    let Ok(shift) = u32::try_from(rhs) else {
        return Err(ConstArithmeticError::ShiftTooLarge);
    };

    let value = if is_left {
        (lhs as u128).checked_shl(shift).map(|value| value as i128)
    } else if unsigned_lhs {
        (lhs as u128).checked_shr(shift).map(|value| value as i128)
    } else {
        lhs.checked_shr(shift)
    };

    value.ok_or(ConstArithmeticError::ShiftTooLarge)
}

pub fn eval_binary_values(
    left: ConstValue,
    op: ConstBinaryOp,
    right: ConstValue,
    lhs_is_unsigned: bool,
) -> Result<ConstValue, ConstArithmeticError> {
    match (left, right) {
        (ConstValue::Int(l), ConstValue::Int(r)) => eval_integer_binary(l, op, r, lhs_is_unsigned),
        (ConstValue::Float(l), ConstValue::Float(r)) => eval_float_binary(l, op, r),
        (ConstValue::Bool(l), ConstValue::Bool(r)) => eval_bool_binary(l, op, r),
        (
            ConstValue::Enum {
                tag: l_tag,
                payload: l_payload,
            },
            ConstValue::Enum {
                tag: r_tag,
                payload: r_payload,
            },
        ) => eval_enum_binary(l_tag, l_payload, op, r_tag, r_payload),
        _ => Err(ConstArithmeticError::TypeMismatch),
    }
}

fn eval_integer_binary(
    l: i128,
    op: ConstBinaryOp,
    r: i128,
    lhs_is_unsigned: bool,
) -> Result<ConstValue, ConstArithmeticError> {
    use ConstBinaryOp::*;
    match op {
        Add => Ok(ConstValue::Int(l.wrapping_add(r))),
        Subtract => Ok(ConstValue::Int(l.wrapping_sub(r))),
        Multiply => Ok(ConstValue::Int(l.wrapping_mul(r))),
        Divide => {
            let value = if lhs_is_unsigned {
                eval_const_uint_division(l, r)?
            } else {
                eval_const_int_division(l, r)?
            };
            Ok(ConstValue::Int(value))
        }
        Modulo => {
            let value = if lhs_is_unsigned {
                eval_const_uint_modulo(l, r)?
            } else {
                eval_const_int_modulo(l, r)?
            };
            Ok(ConstValue::Int(value))
        }
        ShiftLeft => Ok(ConstValue::Int(eval_const_int_shift(
            l,
            r,
            true,
            lhs_is_unsigned,
        )?)),
        ShiftRight => Ok(ConstValue::Int(eval_const_int_shift(
            l,
            r,
            false,
            lhs_is_unsigned,
        )?)),
        BitwiseAnd => Ok(ConstValue::Int(l & r)),
        BitwiseOr => Ok(ConstValue::Int(l | r)),
        BitwiseXor => Ok(ConstValue::Int(l ^ r)),
        Equal => Ok(ConstValue::Bool(l == r)),
        NotEqual => Ok(ConstValue::Bool(l != r)),
        LessThan => Ok(ConstValue::Bool(if lhs_is_unsigned {
            (l as u128) < (r as u128)
        } else {
            l < r
        })),
        LessOrEqual => Ok(ConstValue::Bool(if lhs_is_unsigned {
            (l as u128) <= (r as u128)
        } else {
            l <= r
        })),
        GreaterThan => Ok(ConstValue::Bool(if lhs_is_unsigned {
            (l as u128) > (r as u128)
        } else {
            l > r
        })),
        GreaterOrEqual => Ok(ConstValue::Bool(if lhs_is_unsigned {
            (l as u128) >= (r as u128)
        } else {
            l >= r
        })),
        _ => Err(ConstArithmeticError::UnsupportedIntegerOperator),
    }
}

fn eval_float_binary(
    l: f64,
    op: ConstBinaryOp,
    r: f64,
) -> Result<ConstValue, ConstArithmeticError> {
    use ConstBinaryOp::*;
    match op {
        Add => Ok(ConstValue::Float(l + r)),
        Subtract => Ok(ConstValue::Float(l - r)),
        Multiply => Ok(ConstValue::Float(l * r)),
        Divide => Ok(ConstValue::Float(l / r)),
        Equal => Ok(ConstValue::Bool(l == r)),
        NotEqual => Ok(ConstValue::Bool(l != r)),
        LessThan => Ok(ConstValue::Bool(l < r)),
        LessOrEqual => Ok(ConstValue::Bool(l <= r)),
        GreaterThan => Ok(ConstValue::Bool(l > r)),
        GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
        _ => Err(ConstArithmeticError::UnsupportedFloatOperator),
    }
}

fn eval_bool_binary(
    l: bool,
    op: ConstBinaryOp,
    r: bool,
) -> Result<ConstValue, ConstArithmeticError> {
    use ConstBinaryOp::*;
    match op {
        LogicalAnd => Ok(ConstValue::Bool(l && r)),
        LogicalOr => Ok(ConstValue::Bool(l || r)),
        Equal => Ok(ConstValue::Bool(l == r)),
        NotEqual => Ok(ConstValue::Bool(l != r)),
        _ => Err(ConstArithmeticError::UnsupportedBoolOperator),
    }
}

fn eval_enum_binary(
    l_tag: i128,
    l_payload: Option<Box<ConstValue>>,
    op: ConstBinaryOp,
    r_tag: i128,
    r_payload: Option<Box<ConstValue>>,
) -> Result<ConstValue, ConstArithmeticError> {
    use ConstBinaryOp::*;
    match op {
        Equal => Ok(ConstValue::Bool(l_tag == r_tag && l_payload == r_payload)),
        NotEqual => Ok(ConstValue::Bool(l_tag != r_tag || l_payload != r_payload)),
        _ => Err(ConstArithmeticError::UnsupportedEnumOperator),
    }
}

impl<T> Default for ConstExecState<T> {
    fn default() -> Self {
        Self {
            expected_types: Vec::new(),
            function_return_types: Vec::new(),
            return_value: None,
            function_depth: 0,
            loop_depth: 0,
            loop_control: None,
        }
    }
}

impl<T: Copy> ConstExecState<T> {
    pub fn enter_function(&mut self, return_ty: T, has_generic_substs: bool) -> ConstFunctionFrame {
        let frame = ConstFunctionFrame {
            saved_loop_depth: self.loop_depth,
            saved_loop_control: self.loop_control.take(),
            saved_return: self.return_value.take(),
            has_generic_substs,
        };

        self.function_depth += 1;
        self.loop_depth = 0;
        self.function_return_types.push(return_ty);
        frame
    }

    pub fn leave_function(&mut self, frame: ConstFunctionFrame) -> Option<ConstValue> {
        let _ = self.function_return_types.pop();
        let fn_return = self.return_value.take();
        self.return_value = frame.saved_return;
        self.loop_depth = frame.saved_loop_depth;
        self.loop_control = frame.saved_loop_control;
        self.function_depth = self.function_depth.saturating_sub(1);
        fn_return
    }

    pub fn enter_loop(&mut self) {
        self.loop_depth += 1;
    }

    pub fn leave_loop(&mut self) {
        self.loop_depth = self.loop_depth.saturating_sub(1);
    }

    pub fn in_function(&self) -> bool {
        self.function_depth > 0
    }

    pub fn in_loop(&self) -> bool {
        self.loop_depth > 0
    }

    pub fn current_function_depth(&self) -> usize {
        self.function_depth
    }

    pub fn return_value(&self) -> Option<&ConstValue> {
        self.return_value.as_ref()
    }

    pub fn has_return_value(&self) -> bool {
        self.return_value.is_some()
    }

    pub fn set_return_value(&mut self, value: ConstValue) {
        self.return_value = Some(value);
    }

    pub fn current_return_type(&self) -> Option<T> {
        self.function_return_types.last().copied()
    }

    pub fn push_expected_type(&mut self, ty: T) {
        self.expected_types.push(ty);
    }

    pub fn pop_expected_type(&mut self) {
        let _ = self.expected_types.pop();
    }

    pub fn current_expected_type(&self) -> Option<T> {
        self.expected_types.last().copied()
    }

    pub fn loop_control(&self) -> Option<LoopControl> {
        self.loop_control
    }

    pub fn take_loop_control(&mut self) -> Option<LoopControl> {
        self.loop_control.take()
    }

    pub fn set_loop_control(&mut self, control: LoopControl) {
        self.loop_control = Some(control);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstPlaceError {
    MissingField(SymbolId),
    FieldOnNonStruct,
    IndexOutOfBounds,
    StringIndexOutOfBounds,
    IndexOnNonArray,
    ImmutablePointer,
    ExpectedPointer,
}

/// Opaque failure marker for compile-time evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstEvalError;

pub type ConstEvalResult<T> = Result<T, ConstEvalError>;

pub trait ConstEvalHost {
    fn emit_error(&mut self, span: Span, message: String);
    fn emit_error_with_hints(&mut self, span: Span, message: String, hints: &[String]);
    fn resolve_symbol(&self, symbol: SymbolId) -> String;
    fn ty_to_string(&self, ty: TypeId) -> String;
    fn normalize_type(&self, ty: TypeId) -> TypeId;
    fn type_kind(&self, ty: TypeId) -> TypeKind;
    fn layout_size(&mut self, ty: TypeId) -> ConstEvalResult<u64>;
    fn layout_align(&mut self, ty: TypeId) -> ConstEvalResult<u64>;
    fn source_location_value(&mut self, span: Span) -> ConstValue;
}

/// Host callback surface for explicitly hosted compile-time execution.
pub trait ScriptHost {
    fn call_extern(
        &mut self,
        name: &str,
        args: &[ConstValue],
        span: Span,
    ) -> Result<ConstValue, String>;
}

#[derive(Clone, Copy)]
pub struct ScriptHostHandle {
    data: *mut (),
    call_extern: unsafe fn(*mut (), &str, &[ConstValue], Span) -> Result<ConstValue, String>,
}

impl ScriptHostHandle {
    pub fn new<H: ScriptHost>(host: &mut H) -> Self {
        Self {
            data: host as *mut H as *mut (),
            call_extern: call_script_host::<H>,
        }
    }

    pub fn call_extern(
        self,
        name: &str,
        args: &[ConstValue],
        span: Span,
    ) -> Result<ConstValue, String> {
        unsafe { (self.call_extern)(self.data, name, args, span) }
    }
}

unsafe fn call_script_host<H: ScriptHost>(
    data: *mut (),
    name: &str,
    args: &[ConstValue],
    span: Span,
) -> Result<ConstValue, String> {
    unsafe { (&mut *(data as *mut H)).call_extern(name, args, span) }
}

pub fn project_const_value(
    value: &ConstValue,
    path: &[PlaceSegment],
) -> Result<ConstValue, ConstPlaceError> {
    if path.is_empty() {
        return Ok(value.clone());
    }

    match path[0] {
        PlaceSegment::Field(field) => match value {
            ConstValue::Struct(map) => {
                let Some(next) = map.get(&field) else {
                    return Err(ConstPlaceError::MissingField(field));
                };
                project_const_value(next, &path[1..])
            }
            _ => Err(ConstPlaceError::FieldOnNonStruct),
        },
        PlaceSegment::Index(index) => match value {
            ConstValue::Array(items) => {
                let Some(next) = items.get(index) else {
                    return Err(ConstPlaceError::IndexOutOfBounds);
                };
                project_const_value(next, &path[1..])
            }
            ConstValue::String(text) => {
                let Some(byte) = text.as_bytes().get(index) else {
                    return Err(ConstPlaceError::StringIndexOutOfBounds);
                };
                project_const_value(&ConstValue::Int(*byte as i128), &path[1..])
            }
            _ => Err(ConstPlaceError::IndexOnNonArray),
        },
    }
}

pub fn project_const_value_mut<'a>(
    value: &'a mut ConstValue,
    path: &[PlaceSegment],
) -> Result<&'a mut ConstValue, ConstPlaceError> {
    if path.is_empty() {
        return Ok(value);
    }

    match path[0] {
        PlaceSegment::Field(field) => match value {
            ConstValue::Struct(map) => {
                let Some(next) = map.get_mut(&field) else {
                    return Err(ConstPlaceError::MissingField(field));
                };
                project_const_value_mut(next, &path[1..])
            }
            _ => Err(ConstPlaceError::FieldOnNonStruct),
        },
        PlaceSegment::Index(index) => match value {
            ConstValue::Array(items) => {
                let Some(next) = items.get_mut(index) else {
                    return Err(ConstPlaceError::IndexOutOfBounds);
                };
                project_const_value_mut(next, &path[1..])
            }
            _ => Err(ConstPlaceError::IndexOnNonArray),
        },
    }
}
