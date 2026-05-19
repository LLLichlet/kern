//! Control-flow expression checking.
//!
//! Blocks, `if`, `while`, `for`, `match`, `return`, `defer`, and pattern
//! exhaustiveness live here. The match checker builds constructor/scalar
//! coverage state so diagnostics can distinguish unreachable arms from missing
//! cases.

use super::ExprChecker;
use crate::LayoutEngine;
use crate::checker::{ConstEvaluator, ConstValue};
use crate::def::{Def, ImportDef};
use crate::passes::ImportResolver;
use crate::ty::{AnonymousField, PrimitiveType, TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind, StmtKind};
use kernc_utils::{DiagnosticCode, DiagnosticTag, Span, SymbolId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CoveragePattern {
    Wildcard,
    Constructor(CoverageConstructorKind, Vec<CoveragePattern>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CoverageConstructorKind {
    Bool(bool),
    EnumVariant(SymbolId),
    Struct(Vec<SymbolId>),
}

#[derive(Debug, Clone)]
pub(super) struct CoverageConstructor {
    kind: CoverageConstructorKind,
    arg_tys: Vec<TypeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SignedInterval {
    start: i128,
    end: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnsignedInterval {
    start: u128,
    end: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarPoint {
    Signed(i128),
    Unsigned(u128),
}

#[derive(Debug, Clone)]
enum ScalarIntervals {
    Signed(Vec<SignedInterval>),
    Unsigned(Vec<UnsignedInterval>),
}

#[derive(Debug, Clone)]
enum ScalarCoverageState {
    Signed {
        min: i128,
        max: i128,
        covered: Vec<SignedInterval>,
    },
    Unsigned {
        min: u128,
        max: u128,
        covered: Vec<UnsignedInterval>,
    },
}

struct MatchArmCheckState<'a> {
    norm_target: TypeId,
    has_constructor_coverage: bool,
    common_ret_ty: Option<TypeId>,
    seen_patterns: &'a mut Vec<Vec<CoveragePattern>>,
    scalar_coverage: Option<&'a mut ScalarCoverageState>,
    match_closed: &'a mut bool,
    has_catch_all: &'a mut bool,
}

#[derive(Debug, Clone, Eq)]
struct PatternBindField {
    name: SymbolId,
    name_span: Span,
    ty: TypeId,
    is_mut: bool,
}

impl PartialEq for PatternBindField {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.ty == other.ty && self.is_mut == other.is_mut
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatternBindShape {
    fields: Vec<PatternBindField>,
    ty: TypeId,
}

#[derive(Debug, Clone)]
enum CoverageWitness {
    Wildcard,
    Bool(bool),
    EnumVariant {
        name: SymbolId,
        payload: Option<Box<CoverageWitness>>,
    },
    Struct(Vec<(SymbolId, CoverageWitness)>),
}

impl CoverageWitness {
    fn format(&self, checker: &ExprChecker<'_, '_>) -> String {
        match self {
            Self::Wildcard => "_".to_string(),
            Self::Bool(value) => value.to_string(),
            Self::EnumVariant { name, payload } => {
                let name = checker.ctx.resolve(*name).to_string();
                match payload {
                    Some(payload) => format!(".{{ {}: {} }}", name, payload.format(checker)),
                    None => format!(".{}", name),
                }
            }
            Self::Struct(fields) => {
                let fields = fields
                    .iter()
                    .map(|(name, witness)| {
                        format!(
                            "{}: {}",
                            checker.ctx.resolve(*name),
                            witness.format(checker)
                        )
                    })
                    .collect::<Vec<_>>();
                format!(".{{ {} }}", fields.join(", "))
            }
        }
    }
}

impl ScalarIntervals {
    fn is_empty(&self) -> bool {
        match self {
            Self::Signed(intervals) => intervals.is_empty(),
            Self::Unsigned(intervals) => intervals.is_empty(),
        }
    }
}

impl ScalarCoverageState {
    fn new_signed(min: i128, max: i128) -> Self {
        Self::Signed {
            min,
            max,
            covered: Vec::new(),
        }
    }

    fn new_unsigned(min: u128, max: u128) -> Self {
        Self::Unsigned {
            min,
            max,
            covered: Vec::new(),
        }
    }

    fn is_full(&self) -> bool {
        match self {
            Self::Signed { min, max, covered } => {
                covered.len() == 1 && covered[0].start == *min && covered[0].end == *max
            }
            Self::Unsigned { min, max, covered } => {
                covered.len() == 1 && covered[0].start == *min && covered[0].end == *max
            }
        }
    }

    fn covers_all(&self, intervals: &ScalarIntervals) -> bool {
        match (self, intervals) {
            (Self::Signed { covered, .. }, ScalarIntervals::Signed(intervals)) => {
                intervals.iter().all(|interval| {
                    covered
                        .iter()
                        .any(|seen| seen.start <= interval.start && interval.end <= seen.end)
                })
            }
            (Self::Unsigned { covered, .. }, ScalarIntervals::Unsigned(intervals)) => {
                intervals.iter().all(|interval| {
                    covered
                        .iter()
                        .any(|seen| seen.start <= interval.start && interval.end <= seen.end)
                })
            }
            _ => false,
        }
    }

    fn add_intervals(&mut self, intervals: &ScalarIntervals) {
        match (self, intervals) {
            (Self::Signed { covered, .. }, ScalarIntervals::Signed(intervals)) => {
                for interval in intervals {
                    insert_signed_interval(covered, *interval);
                }
            }
            (Self::Unsigned { covered, .. }, ScalarIntervals::Unsigned(intervals)) => {
                for interval in intervals {
                    insert_unsigned_interval(covered, *interval);
                }
            }
            _ => {}
        }
    }

    fn first_uncovered(&self) -> Option<ScalarPoint> {
        match self {
            Self::Signed { min, max, covered } => {
                let mut cursor = *min;
                for interval in covered {
                    if cursor < interval.start {
                        return Some(ScalarPoint::Signed(cursor));
                    }
                    let next_cursor = interval.end.checked_add(1)?;
                    cursor = next_cursor;
                    if cursor > *max {
                        return None;
                    }
                }

                (cursor <= *max).then_some(ScalarPoint::Signed(cursor))
            }
            Self::Unsigned { min, max, covered } => {
                let mut cursor = *min;
                for interval in covered {
                    if cursor < interval.start {
                        return Some(ScalarPoint::Unsigned(cursor));
                    }
                    let next_cursor = interval.end.checked_add(1)?;
                    cursor = next_cursor;
                    if cursor > *max {
                        return None;
                    }
                }

                (cursor <= *max).then_some(ScalarPoint::Unsigned(cursor))
            }
        }
    }
}

fn insert_signed_interval(covered: &mut Vec<SignedInterval>, mut next: SignedInterval) {
    if next.end < next.start {
        return;
    }

    let mut index = 0;
    while index < covered.len() {
        let current = covered[index];
        if next.end.saturating_add(1) < current.start {
            break;
        }
        if current.end.saturating_add(1) < next.start {
            index += 1;
            continue;
        }

        next.start = next.start.min(current.start);
        next.end = next.end.max(current.end);
        covered.remove(index);
    }

    covered.insert(index, next);
}

fn insert_unsigned_interval(covered: &mut Vec<UnsignedInterval>, mut next: UnsignedInterval) {
    if next.end < next.start {
        return;
    }

    let mut index = 0;
    while index < covered.len() {
        let current = covered[index];
        if next.end.saturating_add(1) < current.start {
            break;
        }
        if current.end.saturating_add(1) < next.start {
            index += 1;
            continue;
        }

        next.start = next.start.min(current.start);
        next.end = next.end.max(current.end);
        covered.remove(index);
    }

    covered.insert(index, next);
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn reject_returned_capturing_closure(
        &mut self,
        expr: &Expr,
        expected_ty: TypeId,
        actual_ty: TypeId,
    ) -> bool {
        let expected_norm = self.resolve_tv(expected_ty);
        let actual_norm = self.resolve_tv(actual_ty);

        let TypeKind::Pointer { elem, .. } = self.ctx.type_registry.get(expected_norm).clone()
        else {
            return false;
        };
        let expected_elem_norm = self.resolve_tv(elem);
        if !matches!(
            self.ctx.type_registry.get(expected_elem_norm),
            TypeKind::ClosureInterface { .. }
        ) {
            return false;
        }

        let TypeKind::AnonymousState { captures, .. } =
            self.ctx.type_registry.get(actual_norm).clone()
        else {
            return false;
        };
        if captures.is_empty() {
            return false;
        }

        let capture_noun = if captures.len() == 1 {
            "one captured value"
        } else {
            "captured values"
        };
        let expected_str = self.ctx.ty_to_string(expected_ty);
        self.ctx
            .struct_error(
                expr.span,
                format!(
                    "cannot return a capturing closure as `{}`",
                    expected_str
                ),
            )
            .with_span_label(
                expr.span,
                "this closure environment would escape the current stack frame",
            )
            .with_hint(format!(
                "the closure captures {}, so its environment is stored in the current function's stack frame",
                capture_noun
            ))
            .with_hint(format!(
                "returning `{}` here would leave the closure environment dangling after the function returns",
                expected_str
            ))
            .with_hint(
                "return a non-capturing closure, or move the captured state into an explicit object that outlives the callback",
            )
            .emit();
        true
    }

    pub(crate) fn match_enum_def(
        &mut self,
        def_id: crate::def::DefId,
        span: Span,
        context: &str,
    ) -> Option<*const crate::def::EnumDef> {
        match self.ctx.defs.get(def_id.0 as usize) {
            Some(Def::Enum(def)) => Some(std::ptr::from_ref(def)),
            Some(other) => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Expected enum definition while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
            None => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Missing DefId {} while trying to {}.",
                        def_id.0, context
                    ),
                );
                None
            }
        }
    }

    fn coverage_bool_constructors(
        &mut self,
        target_ty: TypeId,
    ) -> Option<Vec<CoverageConstructor>> {
        (self.resolve_tv(target_ty) == TypeId::BOOL).then(|| {
            vec![
                CoverageConstructor {
                    kind: CoverageConstructorKind::Bool(false),
                    arg_tys: Vec::new(),
                },
                CoverageConstructor {
                    kind: CoverageConstructorKind::Bool(true),
                    arg_tys: Vec::new(),
                },
            ]
        })
    }

    fn coverage_struct_constructor(&mut self, target_ty: TypeId) -> Option<CoverageConstructor> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Def(def_id, generic_args) => {
                let Def::Struct(def) = self.ctx.defs[def_id.0 as usize].clone() else {
                    return None;
                };
                let generic_map = self.positional_generic_subst_map(&def.generics, &generic_args);
                let mut field_names = Vec::with_capacity(def.fields.len());
                let mut field_tys = Vec::with_capacity(def.fields.len());
                for field in &def.fields {
                    let field_ty = self.ctx.node_type_or_error(field.type_node.id);
                    field_names.push(field.name);
                    field_tys
                        .push(self.substitute_type_with_generic_arg_map(field_ty, &generic_map));
                }

                Some(CoverageConstructor {
                    kind: CoverageConstructorKind::Struct(field_names),
                    arg_tys: field_tys,
                })
            }
            TypeKind::AnonymousStruct(_, fields) => Some(CoverageConstructor {
                kind: CoverageConstructorKind::Struct(
                    fields.iter().map(|field| field.name).collect::<Vec<_>>(),
                ),
                arg_tys: fields.iter().map(|field| field.ty).collect::<Vec<_>>(),
            }),
            _ => None,
        }
    }

    fn coverage_enum_constructors(
        &mut self,
        target_ty: TypeId,
    ) -> Option<Vec<CoverageConstructor>> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Enum(def_id, generic_args) => {
                let adt_def =
                    self.match_enum_def(def_id, Span::default(), "inspect enum coverage")?;
                // SAFETY: semantic defs are immutable while type checking expressions.
                let adt_def = unsafe { &*adt_def }.clone();
                if adt_def.is_extern {
                    return None;
                }
                let generic_map =
                    self.positional_generic_subst_map(&adt_def.generics, &generic_args);
                Some(
                    adt_def
                        .variants
                        .iter()
                        .map(|variant| {
                            let arg_tys = variant
                                .payload_type
                                .as_ref()
                                .map(|payload| {
                                    let ty = self.ctx.node_type_or_error(payload.id);
                                    vec![
                                        self.substitute_type_with_generic_arg_map(ty, &generic_map),
                                    ]
                                })
                                .unwrap_or_default();
                            CoverageConstructor {
                                kind: CoverageConstructorKind::EnumVariant(variant.name),
                                arg_tys,
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            }
            TypeKind::AnonymousEnum(enum_def) => Some(
                enum_def
                    .variants
                    .iter()
                    .map(|variant| CoverageConstructor {
                        kind: CoverageConstructorKind::EnumVariant(variant.name),
                        arg_tys: variant.payload_ty.into_iter().collect::<Vec<_>>(),
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        }
    }

    pub(super) fn coverage_constructors(
        &mut self,
        target_ty: TypeId,
    ) -> Option<Vec<CoverageConstructor>> {
        if let Some(bool_ctors) = self.coverage_bool_constructors(target_ty) {
            return Some(bool_ctors);
        }

        if let Some(enum_ctors) = self.coverage_enum_constructors(target_ty) {
            return Some(enum_ctors);
        }

        self.coverage_struct_constructor(target_ty)
            .map(|ctor| vec![ctor])
    }

    pub(super) fn coverage_lower_pattern(
        &mut self,
        pattern: &ast::Pattern,
        target_ty: TypeId,
    ) -> Option<CoveragePattern> {
        let norm_target = self.resolve_tv(target_ty);
        match &pattern.kind {
            ast::PatternKind::Binding(_) | ast::PatternKind::Ignore => {
                Some(CoveragePattern::Wildcard)
            }
            ast::PatternKind::Variant(variant) => Some(CoveragePattern::Constructor(
                CoverageConstructorKind::EnumVariant(variant.variant_name),
                Vec::new(),
            )),
            ast::PatternKind::Destructure(destructure) => {
                if let Some(enum_ctors) = self.coverage_enum_constructors(norm_target)
                    && destructure.fields.len() == 1
                {
                    let field = &destructure.fields[0];
                    let ctor = enum_ctors.into_iter().find(|ctor| {
                        ctor.kind == CoverageConstructorKind::EnumVariant(field.name)
                    })?;
                    let args = if let Some(&payload_ty) = ctor.arg_tys.first() {
                        vec![self.coverage_lower_pattern(&field.pattern, payload_ty)?]
                    } else {
                        Vec::new()
                    };
                    return Some(CoveragePattern::Constructor(ctor.kind, args));
                }

                let ctor = self.coverage_struct_constructor(norm_target)?;
                let CoverageConstructorKind::Struct(field_names) = ctor.kind.clone() else {
                    self.ctx.emit_ice(
                        pattern.span,
                        "Kern ICE (Typeck): expected struct constructor while lowering match coverage",
                    );
                    return None;
                };

                let mut args = Vec::with_capacity(field_names.len());
                for (index, field_name) in field_names.iter().enumerate() {
                    let lowered = if let Some(field) = destructure
                        .fields
                        .iter()
                        .find(|field| field.name == *field_name)
                    {
                        self.coverage_lower_pattern(&field.pattern, ctor.arg_tys[index])?
                    } else {
                        CoveragePattern::Wildcard
                    };
                    args.push(lowered);
                }

                Some(CoveragePattern::Constructor(
                    CoverageConstructorKind::Struct(field_names),
                    args,
                ))
            }
        }
    }

    fn coverage_lower_expr_pattern(
        &mut self,
        expr: &Expr,
        target_ty: TypeId,
    ) -> Option<CoveragePattern> {
        let norm_target = self.resolve_tv(target_ty);
        match &expr.kind {
            ExprKind::Grouped { expr, .. } => self.coverage_lower_expr_pattern(expr, target_ty),
            ExprKind::Bool(value) if norm_target == TypeId::BOOL => Some(
                CoveragePattern::Constructor(CoverageConstructorKind::Bool(*value), Vec::new()),
            ),
            ExprKind::EnumLiteral { variant, .. } => Some(CoveragePattern::Constructor(
                CoverageConstructorKind::EnumVariant(*variant),
                Vec::new(),
            )),
            ExprKind::FieldAccess { field, .. }
                if self.expr_pattern_is_qualified_enum_variant(expr, target_ty) =>
            {
                Some(CoveragePattern::Constructor(
                    CoverageConstructorKind::EnumVariant(*field),
                    Vec::new(),
                ))
            }
            ExprKind::DataInit {
                literal: kernc_ast::DataLiteralKind::Struct(fields),
                ..
            } => {
                if let [field] = fields.as_slice()
                    && let Some(ctor) =
                        self.coverage_enum_constructors(norm_target)
                            .and_then(|ctors| {
                                ctors.into_iter().find(|ctor| {
                                    ctor.kind == CoverageConstructorKind::EnumVariant(field.name)
                                })
                            })
                {
                    let args = if let Some(&payload_ty) = ctor.arg_tys.first() {
                        vec![self.coverage_lower_expr_pattern(&field.value, payload_ty)?]
                    } else {
                        Vec::new()
                    };
                    return Some(CoveragePattern::Constructor(ctor.kind, args));
                }

                let ctor = self.coverage_struct_constructor(norm_target)?;
                let CoverageConstructorKind::Struct(field_names) = ctor.kind.clone() else {
                    self.ctx.emit_ice(
                        expr.span,
                        "Kern ICE (Typeck): expected struct constructor while lowering value-pattern coverage",
                    );
                    return None;
                };

                let mut args = Vec::with_capacity(field_names.len());
                for (index, field_name) in field_names.iter().enumerate() {
                    let field = fields.iter().find(|field| field.name == *field_name)?;
                    args.push(self.coverage_lower_expr_pattern(&field.value, ctor.arg_tys[index])?);
                }

                Some(CoveragePattern::Constructor(
                    CoverageConstructorKind::Struct(field_names),
                    args,
                ))
            }
            _ => None,
        }
    }

    fn expr_pattern_is_qualified_enum_variant(&mut self, expr: &Expr, target_ty: TypeId) -> bool {
        let ExprKind::FieldAccess { lhs, .. } = &expr.kind else {
            return false;
        };
        let Some(lhs_ty) = self.ctx.node_type(lhs.id) else {
            return false;
        };

        let lhs_resolved = self.resolve_tv(lhs_ty);
        let target_resolved = self.resolve_tv(target_ty);
        let lhs_norm = self.ctx.normalize_concrete_type(lhs_resolved);
        let target_norm = self.ctx.normalize_concrete_type(target_resolved);
        if lhs_norm != target_norm {
            return false;
        }

        matches!(
            self.ctx.type_registry.get(target_norm),
            TypeKind::Enum(..) | TypeKind::AnonymousEnum(_)
        )
    }

    fn coverage_lower_match_pattern(
        &mut self,
        pattern: &ast::MatchPattern,
        target_ty: TypeId,
    ) -> Option<CoveragePattern> {
        match &pattern.kind {
            ast::MatchPatternKind::Pattern(pattern) => {
                self.coverage_lower_pattern(pattern, target_ty)
            }
            ast::MatchPatternKind::Value(expr) => self.coverage_lower_expr_pattern(expr, target_ty),
        }
    }

    fn specialize_coverage_pattern(
        &self,
        pattern: &CoveragePattern,
        ctor: &CoverageConstructor,
    ) -> Option<Vec<CoveragePattern>> {
        match pattern {
            CoveragePattern::Wildcard => Some(vec![CoveragePattern::Wildcard; ctor.arg_tys.len()]),
            CoveragePattern::Constructor(kind, args) if *kind == ctor.kind => Some(args.clone()),
            CoveragePattern::Constructor(_, _) => None,
        }
    }

    fn coverage_default_matrix(
        &self,
        matrix: &[Vec<CoveragePattern>],
    ) -> Vec<Vec<CoveragePattern>> {
        matrix
            .iter()
            .filter_map(|row| match row.first() {
                Some(CoveragePattern::Wildcard) => Some(row[1..].to_vec()),
                _ => None,
            })
            .collect()
    }

    fn coverage_specialize_matrix(
        &self,
        matrix: &[Vec<CoveragePattern>],
        ctor: &CoverageConstructor,
    ) -> Vec<Vec<CoveragePattern>> {
        matrix
            .iter()
            .filter_map(|row| {
                let head = row.first()?;
                let mut specialized = self.specialize_coverage_pattern(head, ctor)?;
                specialized.extend_from_slice(&row[1..]);
                Some(specialized)
            })
            .collect()
    }

    fn coverage_rebuild_witness(
        &self,
        ctor: &CoverageConstructor,
        parts: &mut Vec<CoverageWitness>,
    ) -> CoverageWitness {
        let mut ctor_parts = parts.drain(..ctor.arg_tys.len()).collect::<Vec<_>>();
        match &ctor.kind {
            CoverageConstructorKind::Bool(value) => CoverageWitness::Bool(*value),
            CoverageConstructorKind::EnumVariant(name) => CoverageWitness::EnumVariant {
                name: *name,
                payload: ctor_parts.pop().map(Box::new),
            },
            CoverageConstructorKind::Struct(field_names) => CoverageWitness::Struct(
                field_names
                    .iter()
                    .copied()
                    .zip(ctor_parts)
                    .collect::<Vec<_>>(),
            ),
        }
    }

    fn coverage_has_wildcard_cover(&self, matrix: &[Vec<CoveragePattern>], width: usize) -> bool {
        // A fully-wildcard row already covers every value in the remaining columns; expanding it
        // through nested payload and product types only repeats that proof at much higher cost.
        matrix.iter().any(|row| {
            row.len() == width
                && row
                    .iter()
                    .all(|pattern| matches!(pattern, CoveragePattern::Wildcard))
        })
    }

    pub(super) fn coverage_matrix_is_exhaustive(
        &mut self,
        target_ty: TypeId,
        matrix: &[Vec<CoveragePattern>],
    ) -> bool {
        self.coverage_find_uncovered_vector(&[target_ty], matrix)
            .is_none()
    }

    fn scalar_integer_kind(&mut self, target_ty: TypeId) -> Option<(bool, u64)> {
        let norm_target = self.resolve_tv(target_ty);
        let TypeKind::Primitive(primitive) = self.ctx.type_registry.get(norm_target) else {
            return None;
        };

        let is_unsigned = matches!(
            primitive,
            PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::U128
                | PrimitiveType::USize
        );
        let is_signed = matches!(
            primitive,
            PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::I128
                | PrimitiveType::ISize
        );
        if !is_unsigned && !is_signed {
            return None;
        }

        let bit_width = LayoutEngine::new(self.ctx).compute_type_size(norm_target) * 8;
        Some((is_unsigned, bit_width))
    }

    fn scalar_domain(&mut self, target_ty: TypeId) -> Option<ScalarCoverageState> {
        let norm_target = self.resolve_tv(target_ty);
        if norm_target == TypeId::BOOL {
            return Some(ScalarCoverageState::new_unsigned(0, 1));
        }

        let (is_unsigned, bit_width) = self.scalar_integer_kind(norm_target)?;
        if is_unsigned {
            let max = if bit_width >= 128 {
                u128::MAX
            } else {
                (1u128 << bit_width) - 1
            };
            Some(ScalarCoverageState::new_unsigned(0, max))
        } else {
            let (min, max) = if bit_width >= 128 {
                (i128::MIN, i128::MAX)
            } else {
                let max = ((1u128 << (bit_width - 1)) - 1) as i128;
                let min = -(1i128 << (bit_width - 1));
                (min, max)
            };
            Some(ScalarCoverageState::new_signed(min, max))
        }
    }

    fn scalar_const_value(&mut self, expr: &Expr) -> Option<ConstValue> {
        ConstEvaluator::new(self.ctx).eval_const_value(expr).ok()
    }

    fn scalar_value_point(&mut self, value: ConstValue, target_ty: TypeId) -> Option<ScalarPoint> {
        let norm_target = self.resolve_tv(target_ty);
        match value {
            ConstValue::Bool(value) if norm_target == TypeId::BOOL => {
                Some(ScalarPoint::Unsigned(if value { 1 } else { 0 }))
            }
            ConstValue::Int(value) if self.ctx.type_registry.is_integer(norm_target) => {
                let (is_unsigned, _) = self.scalar_integer_kind(norm_target)?;
                if is_unsigned {
                    Some(ScalarPoint::Unsigned(value as u128))
                } else {
                    Some(ScalarPoint::Signed(value))
                }
            }
            _ => None,
        }
    }

    fn scalar_pattern_intervals(
        &mut self,
        pattern: &ast::MatchPattern,
        target_ty: TypeId,
        coverage: &ScalarCoverageState,
    ) -> Option<ScalarIntervals> {
        match &pattern.kind {
            ast::MatchPatternKind::Value(expr) => {
                if self.ctx.match_value_pattern_bind_ty(expr.id).is_some() {
                    return None;
                }
                if let ExprKind::Range {
                    start: Some(start),
                    end: Some(end),
                    is_inclusive,
                } = &expr.kind
                {
                    return self.scalar_range_intervals(
                        start,
                        end,
                        *is_inclusive,
                        target_ty,
                        coverage,
                    );
                }

                let value = self.scalar_const_value(expr)?;
                let point = self.scalar_value_point(value, target_ty)?;
                match (coverage, point) {
                    (ScalarCoverageState::Signed { min, max, .. }, ScalarPoint::Signed(point)) => {
                        if point < *min || point > *max {
                            return Some(ScalarIntervals::Signed(Vec::new()));
                        }
                        Some(ScalarIntervals::Signed(vec![SignedInterval {
                            start: point,
                            end: point,
                        }]))
                    }
                    (
                        ScalarCoverageState::Unsigned { min, max, .. },
                        ScalarPoint::Unsigned(point),
                    ) => {
                        if point < *min || point > *max {
                            return Some(ScalarIntervals::Unsigned(Vec::new()));
                        }
                        Some(ScalarIntervals::Unsigned(vec![UnsignedInterval {
                            start: point,
                            end: point,
                        }]))
                    }
                    _ => None,
                }
            }
            ast::MatchPatternKind::Pattern(_) => None,
        }
    }

    fn scalar_range_intervals(
        &mut self,
        start: &Expr,
        end: &Expr,
        inclusive: bool,
        target_ty: TypeId,
        coverage: &ScalarCoverageState,
    ) -> Option<ScalarIntervals> {
        let start_value = self.scalar_const_value(start)?;
        let end_value = self.scalar_const_value(end)?;
        let start = self.scalar_value_point(start_value, target_ty)?;
        let end = self.scalar_value_point(end_value, target_ty)?;
        match (coverage, start, end) {
            (
                ScalarCoverageState::Signed { min, max, .. },
                ScalarPoint::Signed(start),
                ScalarPoint::Signed(end),
            ) => {
                let end = if inclusive {
                    end
                } else if let Some(end) = end.checked_sub(1) {
                    end
                } else {
                    return Some(ScalarIntervals::Signed(Vec::new()));
                };
                if end < start {
                    return Some(ScalarIntervals::Signed(Vec::new()));
                }
                let start = start.max(*min);
                let end = end.min(*max);
                if end < start {
                    return Some(ScalarIntervals::Signed(Vec::new()));
                }
                Some(ScalarIntervals::Signed(vec![SignedInterval { start, end }]))
            }
            (
                ScalarCoverageState::Unsigned { min, max, .. },
                ScalarPoint::Unsigned(start),
                ScalarPoint::Unsigned(end),
            ) => {
                let end = if inclusive {
                    end
                } else if let Some(end) = end.checked_sub(1) {
                    end
                } else {
                    return Some(ScalarIntervals::Unsigned(Vec::new()));
                };
                if end < start {
                    return Some(ScalarIntervals::Unsigned(Vec::new()));
                }
                let start = start.max(*min);
                let end = end.min(*max);
                if end < start {
                    return Some(ScalarIntervals::Unsigned(Vec::new()));
                }
                Some(ScalarIntervals::Unsigned(vec![UnsignedInterval {
                    start,
                    end,
                }]))
            }
            _ => None,
        }
    }

    fn scalar_witness_string(&self, target_ty: TypeId, value: ScalarPoint) -> String {
        if self.ctx.type_registry.normalize(target_ty) == TypeId::BOOL {
            return match value {
                ScalarPoint::Unsigned(0) => "false".to_string(),
                ScalarPoint::Unsigned(_) => "true".to_string(),
                ScalarPoint::Signed(0) => "false".to_string(),
                ScalarPoint::Signed(_) => "true".to_string(),
            };
        }

        match value {
            ScalarPoint::Signed(value) => value.to_string(),
            ScalarPoint::Unsigned(value) => value.to_string(),
        }
    }

    pub(super) fn coverage_vector_is_useful(
        &mut self,
        tys: &[TypeId],
        matrix: &[Vec<CoveragePattern>],
        vector: &[CoveragePattern],
    ) -> bool {
        if tys.is_empty() {
            return matrix.is_empty();
        }

        if self.coverage_has_wildcard_cover(matrix, tys.len()) {
            return false;
        }

        let head_ty = self.resolve_tv(tys[0]);
        let Some(head_pattern) = vector.first() else {
            return false;
        };

        if let Some(ctors) = self.coverage_constructors(head_ty) {
            match head_pattern {
                CoveragePattern::Wildcard => ctors.into_iter().any(|ctor| {
                    let specialized = self.coverage_specialize_matrix(matrix, &ctor);
                    let mut specialized_vector =
                        vec![CoveragePattern::Wildcard; ctor.arg_tys.len()];
                    specialized_vector.extend_from_slice(&vector[1..]);
                    let mut specialized_tys = ctor.arg_tys.clone();
                    specialized_tys.extend_from_slice(&tys[1..]);
                    self.coverage_vector_is_useful(
                        &specialized_tys,
                        &specialized,
                        &specialized_vector,
                    )
                }),
                CoveragePattern::Constructor(kind, args) => {
                    let Some(ctor) = ctors.into_iter().find(|ctor| ctor.kind == *kind) else {
                        return false;
                    };
                    let specialized = self.coverage_specialize_matrix(matrix, &ctor);
                    let mut specialized_vector = args.clone();
                    specialized_vector.extend_from_slice(&vector[1..]);
                    let mut specialized_tys = ctor.arg_tys.clone();
                    specialized_tys.extend_from_slice(&tys[1..]);
                    self.coverage_vector_is_useful(
                        &specialized_tys,
                        &specialized,
                        &specialized_vector,
                    )
                }
            }
        } else {
            match head_pattern {
                CoveragePattern::Wildcard => {
                    let default_matrix = self.coverage_default_matrix(matrix);
                    self.coverage_vector_is_useful(&tys[1..], &default_matrix, &vector[1..])
                }
                CoveragePattern::Constructor(_, _) => false,
            }
        }
    }

    fn warn_unreachable_match_pattern(&mut self, span: Span) {
        self.ctx
            .struct_warning(span, "unreachable match pattern")
            .with_code(DiagnosticCode::UnreachablePattern)
            .with_tag(DiagnosticTag::Unnecessary)
            .with_hint("previous patterns already cover every value matched by this pattern")
            .emit();
    }

    fn coverage_find_uncovered_vector(
        &mut self,
        tys: &[TypeId],
        matrix: &[Vec<CoveragePattern>],
    ) -> Option<Vec<CoverageWitness>> {
        if tys.is_empty() {
            return matrix.is_empty().then(Vec::new);
        }

        if self.coverage_has_wildcard_cover(matrix, tys.len()) {
            return None;
        }

        let head_ty = self.resolve_tv(tys[0]);
        if let Some(ctors) = self.coverage_constructors(head_ty) {
            for ctor in ctors {
                let specialized = self.coverage_specialize_matrix(matrix, &ctor);
                let mut sub_tys = ctor.arg_tys.clone();
                sub_tys.extend_from_slice(&tys[1..]);
                if let Some(mut uncovered) =
                    self.coverage_find_uncovered_vector(&sub_tys, &specialized)
                {
                    let witness = self.coverage_rebuild_witness(&ctor, &mut uncovered);
                    uncovered.insert(0, witness);
                    return Some(uncovered);
                }
            }
            None
        } else {
            let default_matrix = self.coverage_default_matrix(matrix);
            let mut uncovered = self.coverage_find_uncovered_vector(&tys[1..], &default_matrix)?;
            uncovered.insert(0, CoverageWitness::Wildcard);
            Some(uncovered)
        }
    }

    pub(super) fn uncovered_pattern_witness(
        &mut self,
        target_ty: TypeId,
        patterns: &[&ast::Pattern],
    ) -> Option<String> {
        let matrix = patterns
            .iter()
            .filter_map(|pattern| self.coverage_lower_pattern(pattern, target_ty))
            .map(|pattern| vec![pattern])
            .collect::<Vec<_>>();
        let witness = self.coverage_find_uncovered_vector(&[target_ty], &matrix)?;
        witness.first().map(|witness| witness.format(self))
    }

    fn uncovered_match_witness(
        &mut self,
        target_ty: TypeId,
        arms: &[ast::MatchArm],
    ) -> Option<String> {
        let mut matrix = Vec::new();
        for arm in arms {
            for pattern in &arm.patterns {
                if let Some(lowered) = self.coverage_lower_match_pattern(pattern, target_ty) {
                    matrix.push(vec![lowered]);
                }
            }
        }

        let witness = self.coverage_find_uncovered_vector(&[target_ty], &matrix)?;
        witness.first().map(|witness| witness.format(self))
    }

    fn void_pattern_bind_shape(&self) -> PatternBindShape {
        PatternBindShape {
            fields: Vec::new(),
            ty: TypeId::VOID,
        }
    }

    fn pattern_bind_shape_from_fields(
        &mut self,
        mut fields: Vec<PatternBindField>,
    ) -> PatternBindShape {
        if fields.is_empty() {
            return self.void_pattern_bind_shape();
        }

        fields.sort_by_key(|field| field.name);
        let struct_fields = fields
            .iter()
            .map(|field| AnonymousField {
                name: field.name,
                ty: field.ty,
            })
            .collect();
        let ty = self
            .ctx
            .type_registry
            .intern(TypeKind::AnonymousStruct(false, struct_fields));
        PatternBindShape { fields, ty }
    }

    fn pattern_bind_shape(
        &mut self,
        pattern: &ast::Pattern,
        actual_ty: TypeId,
    ) -> PatternBindShape {
        match &pattern.kind {
            ast::PatternKind::Binding(binding) => {
                if self.ctx.resolve(binding.name) == "_" {
                    self.void_pattern_bind_shape()
                } else {
                    self.pattern_bind_shape_from_fields(vec![PatternBindField {
                        name: binding.name,
                        name_span: binding.name_span,
                        ty: actual_ty,
                        is_mut: binding.is_mut,
                    }])
                }
            }
            ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {
                self.void_pattern_bind_shape()
            }
            ast::PatternKind::Destructure(destructure) => {
                let norm_target = self.resolve_tv(actual_ty);
                match self.ctx.type_registry.get(norm_target).clone() {
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_) => {
                        let Some(field) = destructure.fields.first() else {
                            return self.void_pattern_bind_shape();
                        };
                        let Some(Some(payload_ty)) =
                            self.variant_payload_type(norm_target, field.name, field.name_span)
                        else {
                            return self.void_pattern_bind_shape();
                        };
                        self.pattern_bind_shape(&field.pattern, payload_ty)
                    }
                    _ => {
                        let Some((field_defs, _)) =
                            self.resolve_struct_pattern_fields(norm_target, pattern.span)
                        else {
                            return self.void_pattern_bind_shape();
                        };

                        let mut fields = Vec::new();
                        for field in &destructure.fields {
                            let Some(resolved) = field_defs
                                .iter()
                                .find(|candidate| candidate.name == field.name)
                            else {
                                continue;
                            };
                            let shape = self.pattern_bind_shape(&field.pattern, resolved.ty);
                            fields.extend(shape.fields);
                        }
                        self.pattern_bind_shape_from_fields(fields)
                    }
                }
            }
        }
    }

    fn match_pattern_bind_shape(
        &mut self,
        pattern: &ast::MatchPattern,
        target_ty: TypeId,
    ) -> PatternBindShape {
        match &pattern.kind {
            ast::MatchPatternKind::Pattern(pattern) => self.pattern_bind_shape(pattern, target_ty),
            ast::MatchPatternKind::Value(value) => self
                .ctx
                .match_value_pattern_bind_ty(value.id)
                .map(|bind_ty| self.bind_shape_from_pattern_bind_ty(bind_ty, value.span))
                .unwrap_or_else(|| self.void_pattern_bind_shape()),
        }
    }

    fn bind_shape_from_pattern_bind_ty(&mut self, bind_ty: TypeId, span: Span) -> PatternBindShape {
        let bind_ty = self.ctx.normalize_concrete_type(bind_ty);
        let bind_ty = self.resolve_tv(bind_ty);
        if bind_ty == TypeId::VOID {
            return self.void_pattern_bind_shape();
        }

        match self.ctx.type_registry.get(bind_ty).clone() {
            TypeKind::AnonymousStruct(_, fields) => {
                let fields = fields
                    .into_iter()
                    .map(|field| PatternBindField {
                        name: field.name,
                        name_span: span,
                        ty: field.ty,
                        is_mut: false,
                    })
                    .collect();
                self.pattern_bind_shape_from_fields(fields)
            }
            _ => {
                let bind_str = self.ctx.ty_to_string(bind_ty);
                self.ctx
                    .struct_error(span, "pattern binding type must be `void` or `struct { ... }`")
                    .with_hint(format!("found `Bind = {}`", bind_str))
                    .with_hint(
                        "use `void` for a no-binding pattern, or an anonymous struct whose fields become arm bindings",
                    )
                    .emit();
                self.void_pattern_bind_shape()
            }
        }
    }

    fn pattern_trait_bind_ty(&mut self, pattern_ty: TypeId, target_ty: TypeId) -> Option<TypeId> {
        let trait_def_id = self.ctx.builtin_def("Pattern")?;
        let bind_assoc_id = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => {
                trait_def.assoc_types.iter().copied().find(|assoc_id| {
                    matches!(
                        self.ctx.defs.get(assoc_id.0 as usize),
                        Some(Def::AssociatedType(assoc_def))
                            if self.ctx.resolve(assoc_def.name) == "Bind"
                    )
                })?
            }
            _ => return None,
        };
        let trait_args = vec![crate::ty::GenericArg::Type(target_ty)];
        let trait_ty = self.ctx.builtin_trait_ty("Pattern", vec![target_ty])?;
        if !self.check_trait_impl(pattern_ty, trait_ty) {
            return None;
        }

        if let Some((concrete_target, bind_ty)) = self.pattern_impl_head_bind_ty(
            pattern_ty,
            target_ty,
            trait_def_id,
            &trait_args,
            bind_assoc_id,
        ) {
            let resolved_target = self.resolve_tv(target_ty);
            if let TypeKind::TypeVar(target_vid) =
                self.ctx.type_registry.get(resolved_target).clone()
            {
                self.bind_type_var(target_vid, concrete_target);
            }
            return Some(self.ctx.normalize_concrete_type(bind_ty));
        }

        let target_ty = self.resolve_tv(target_ty);
        let projection = self.ctx.type_registry.intern(TypeKind::Projection {
            target: pattern_ty,
            trait_def_id,
            trait_args: vec![crate::ty::GenericArg::Type(target_ty)],
            assoc_def_id: bind_assoc_id,
            assoc_args: vec![],
        });
        Some(self.ctx.normalize_concrete_type(projection))
    }

    fn pattern_impl_head_bind_ty(
        &mut self,
        pattern_ty: TypeId,
        _target_ty: TypeId,
        trait_def_id: crate::def::DefId,
        trait_args: &[crate::ty::GenericArg],
        bind_assoc_id: crate::def::DefId,
    ) -> Option<(TypeId, TypeId)> {
        let candidates = crate::query::collect_specificity_maximal_trait_impl_head_candidates(
            self.ctx,
            pattern_ty,
            trait_def_id,
            trait_args,
        );
        let [candidate] = candidates.as_slice() else {
            return None;
        };
        let impl_trait_ty = crate::query::instantiate_impl_trait_ty(
            self.ctx,
            candidate.impl_id,
            &candidate.impl_args,
        )?;
        let TypeKind::TraitObject(_, impl_args, assoc_bindings) = self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(impl_trait_ty))
            .clone()
        else {
            return None;
        };
        let Some(crate::ty::GenericArg::Type(concrete_target)) = impl_args.first().copied() else {
            return None;
        };
        let bind_ty = assoc_bindings
            .into_iter()
            .find(|(assoc_id, _)| *assoc_id == bind_assoc_id)
            .map(|(_, ty)| ty)?;
        Some((concrete_target, bind_ty))
    }

    fn value_pattern_is_compiler_known(&mut self, value: &Expr, target_ty: TypeId) -> bool {
        let norm_target = self.resolve_tv(target_ty);
        match &value.kind {
            ExprKind::Grouped { expr, .. } => self.value_pattern_is_compiler_known(expr, target_ty),
            ExprKind::Range { .. } => true,
            ExprKind::Bool(_) => true,
            ExprKind::Integer { .. } | ExprKind::Char(_) | ExprKind::ByteChar(_)
                if self.ctx.type_registry.is_integer(norm_target) =>
            {
                true
            }
            ExprKind::Float { .. } if self.ctx.type_registry.is_float(norm_target) => true,
            ExprKind::Unary {
                op: ast::UnaryOperator::Negate,
                operand,
            } => self.value_pattern_is_compiler_known(operand, target_ty),
            ExprKind::EnumLiteral { .. } => true,
            ExprKind::FieldAccess { .. }
                if matches!(
                    self.ctx.type_registry.get(norm_target),
                    TypeKind::Enum(..) | TypeKind::AnonymousEnum(_)
                ) =>
            {
                true
            }
            ExprKind::DataInit {
                literal: ast::DataLiteralKind::Struct(_),
                type_node,
            } => match type_node {
                Some(type_node) => {
                    let pattern_ty = self.evaluate_dynamic_typeof(type_node);
                    let pattern_ty = self.resolve_tv(pattern_ty);
                    let pattern_ty = self.ctx.normalize_concrete_type(pattern_ty);
                    let target_ty = self.ctx.normalize_concrete_type(norm_target);
                    pattern_ty == target_ty
                }
                None => true,
            },
            _ => false,
        }
    }

    fn check_compiler_known_value_pattern(&mut self, value: &Expr, target_ty: TypeId) {
        let v_ty = self.check_expr(value, Some(target_ty));
        self.check_coercion(value, target_ty, v_ty);
    }

    fn try_user_value_pattern(&mut self, value: &Expr, target_ty: TypeId) -> Option<TypeId> {
        let pattern_ty = self.check_expr(value, None);
        if pattern_ty == TypeId::ERROR {
            return Some(TypeId::ERROR);
        }

        let bind_ty = self.pattern_trait_bind_ty(pattern_ty, target_ty)?;

        self.ctx.set_match_value_pattern_bind_ty(value.id, bind_ty);
        Some(bind_ty)
    }

    fn emit_match_arm_bind_shape_error(
        &mut self,
        span: Span,
        expected: &PatternBindShape,
        found: &PatternBindShape,
    ) {
        let expected_ty = self.ctx.ty_to_string(expected.ty);
        let found_ty = self.ctx.ty_to_string(found.ty);
        self.ctx
            .struct_error(span, "match arm patterns must bind the same names")
            .with_hint(format!("first pattern binds `{}`", expected_ty))
            .with_hint(format!("this pattern binds `{}`", found_ty))
            .emit();
    }

    /// Core match-checking logic, including environment extraction and exhaustiveness.
    pub(crate) fn check_match_expr(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let norm_target = self.resolve_tv(target_ty);

        if norm_target == TypeId::ERROR {
            for arm in arms {
                if self.is_canceled() {
                    return TypeId::ERROR;
                }
                self.check_expr(&arm.body, None);
            }
            return TypeId::ERROR;
        }

        let has_constructor_coverage = self.coverage_constructors(norm_target).is_some();

        let mut common_ret_ty = expected_ty;
        let mut has_catch_all = false;
        let mut seen_patterns = Vec::new();
        let mut scalar_coverage = self.scalar_domain(norm_target);
        let mut match_closed = false;

        for arm in arms {
            if self.is_canceled() {
                return TypeId::ERROR;
            }
            let mut arm_state = MatchArmCheckState {
                norm_target,
                has_constructor_coverage,
                common_ret_ty,
                seen_patterns: &mut seen_patterns,
                scalar_coverage: scalar_coverage.as_mut(),
                match_closed: &mut match_closed,
                has_catch_all: &mut has_catch_all,
            };
            let body_ty = self.check_match_arm(arm, &mut arm_state);

            if common_ret_ty.is_none() || common_ret_ty == Some(TypeId::NEVER) {
                common_ret_ty = Some(body_ty);
            } else if let Some(common_ty) = common_ret_ty.filter(|ty| *ty != TypeId::NEVER)
                && body_ty != TypeId::NEVER
            {
                let body_started = self.timing_start();
                self.check_coercion(&arm.body, common_ty, body_ty);
                self.record_expr_timing(body_started, |stats, elapsed| {
                    stats.control_match_bodies += elapsed;
                });
            }
        }

        // --- Exhaustiveness checking ---
        if !has_catch_all {
            let exhaustiveness_started = self.timing_start();
            if has_constructor_coverage {
                if let Some(witness) = self.uncovered_match_witness(norm_target, arms) {
                    self.ctx
                        .struct_error(span, "match expression is not exhaustive")
                        .with_code(DiagnosticCode::NonexhaustiveMatch)
                        .with_hint(format!(
                            "for example, this value is not covered: `{}`",
                            witness
                        ))
                        .emit();
                }
            } else if let Some(scalar_coverage) = &scalar_coverage {
                if let Some(value) = scalar_coverage.first_uncovered() {
                    let witness = self.scalar_witness_string(norm_target, value);
                    self.ctx
                        .struct_error(span, "match expression is not exhaustive")
                        .with_code(DiagnosticCode::NonexhaustiveMatch)
                        .with_hint(format!(
                            "for example, this value is not covered: `{}`",
                            witness
                        ))
                        .emit();
                }
            } else {
                // Non-ADT matches require a catch-all arm.
                self.ctx
                    .struct_error(span, "match expression must be exhaustive")
                    .with_code(DiagnosticCode::NonexhaustiveMatch)
                    .with_hint("for non-ADT types (like integers or strings), consider adding an `else =>` catch-all branch")
                    .emit();
            }
            self.record_expr_timing(exhaustiveness_started, |stats, elapsed| {
                stats.control_match_exhaustiveness += elapsed;
            });
        }

        common_ret_ty.unwrap_or(TypeId::VOID)
    }

    /// Check a single match arm in isolation.
    fn check_match_arm(
        &mut self,
        arm: &ast::MatchArm,
        state: &mut MatchArmCheckState<'_>,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();

        let pattern_started = self.timing_start();
        let mut arm_bind_shape = None;
        for pat in &arm.patterns {
            if self.is_canceled() {
                self.ctx.scopes.exit_scope();
                return TypeId::ERROR;
            }
            match &pat.kind {
                ast::MatchPatternKind::Value(v) => {
                    if self.check_match_range_value_pattern(v, state.norm_target) {
                    } else if self.value_pattern_is_compiler_known(v, state.norm_target) {
                        self.check_compiler_known_value_pattern(v, state.norm_target);
                    } else if self.try_user_value_pattern(v, state.norm_target).is_some() {
                    } else {
                        let pattern_ty = self.ctx.node_type_or_error(v.id);
                        let pattern_str = self.ctx.ty_to_string(pattern_ty);
                        let target_str = self.ctx.ty_to_string(state.norm_target);
                        self.ctx
                            .struct_error(v.span, "match value is not a valid pattern")
                            .with_hint(format!(
                                "`{}` can be a pattern for `{}` only if it is a compiler-known scalar, enum, struct, or range pattern, or implements `Pattern[{}]`",
                                pattern_str, target_str, target_str
                            ))
                            .emit();
                    }
                    if *state.match_closed {
                        self.warn_unreachable_match_pattern(pat.span);
                    } else if state.has_constructor_coverage
                        && let Some(lowered) =
                            self.coverage_lower_match_pattern(pat, state.norm_target)
                    {
                        if self.coverage_vector_is_useful(
                            &[state.norm_target],
                            state.seen_patterns,
                            std::slice::from_ref(&lowered),
                        ) {
                            state.seen_patterns.push(vec![lowered]);
                            if self.coverage_matrix_is_exhaustive(
                                state.norm_target,
                                state.seen_patterns,
                            ) {
                                *state.has_catch_all = true;
                                *state.match_closed = true;
                            }
                        } else {
                            self.warn_unreachable_match_pattern(pat.span);
                        }
                    } else if let Some(scalar_coverage) = state.scalar_coverage.as_deref_mut()
                        && let Some(intervals) =
                            self.scalar_pattern_intervals(pat, state.norm_target, scalar_coverage)
                    {
                        if intervals.is_empty() || scalar_coverage.covers_all(&intervals) {
                            self.warn_unreachable_match_pattern(pat.span);
                        } else {
                            scalar_coverage.add_intervals(&intervals);
                            if scalar_coverage.is_full() {
                                *state.has_catch_all = true;
                                *state.match_closed = true;
                            }
                        }
                    }
                }
                ast::MatchPatternKind::Pattern(pattern) => {
                    self.check_pattern_without_bindings(arm.body.id, pattern, state.norm_target);

                    let irrefutable = self.pattern_is_irrefutable(pattern, state.norm_target);
                    if *state.match_closed {
                        self.warn_unreachable_match_pattern(pat.span);
                    } else if state.has_constructor_coverage
                        && let Some(lowered) =
                            self.coverage_lower_match_pattern(pat, state.norm_target)
                    {
                        if self.coverage_vector_is_useful(
                            &[state.norm_target],
                            state.seen_patterns,
                            std::slice::from_ref(&lowered),
                        ) {
                            state.seen_patterns.push(vec![lowered]);
                            if irrefutable
                                || self.coverage_matrix_is_exhaustive(
                                    state.norm_target,
                                    state.seen_patterns,
                                )
                            {
                                *state.has_catch_all = true;
                                *state.match_closed = true;
                            }
                        } else {
                            self.warn_unreachable_match_pattern(pat.span);
                        }
                    } else if let Some(scalar_coverage) = state.scalar_coverage.as_deref_mut() {
                        if irrefutable {
                            if scalar_coverage.is_full() {
                                self.warn_unreachable_match_pattern(pat.span);
                            } else {
                                *state.has_catch_all = true;
                                *state.match_closed = true;
                            }
                        }
                    } else if irrefutable {
                        *state.has_catch_all = true;
                        *state.match_closed = true;
                    }
                }
            }

            let bind_shape = self.match_pattern_bind_shape(pat, state.norm_target);
            if let Some(expected) = &arm_bind_shape {
                if expected != &bind_shape {
                    self.emit_match_arm_bind_shape_error(pat.span, expected, &bind_shape);
                }
            } else {
                arm_bind_shape = Some(bind_shape);
            }
        }
        if let Some(bind_shape) = &arm_bind_shape {
            for field in &bind_shape.fields {
                if self.is_canceled() {
                    self.ctx.scopes.exit_scope();
                    return TypeId::ERROR;
                }
                let binding = ast::BindingPattern {
                    name: field.name,
                    name_span: field.name_span,
                    is_mut: field.is_mut,
                    span: field.name_span,
                };
                self.define_pattern_binding(arm.body.id, &binding, field.ty);
            }
        }
        self.record_expr_timing(pattern_started, |stats, elapsed| {
            stats.control_match_patterns += elapsed;
        });

        let body_started = self.timing_start();
        let body_ty = self.check_expr(&arm.body, state.common_ret_ty);
        self.record_expr_timing(body_started, |stats, elapsed| {
            stats.control_match_bodies += elapsed;
        });
        self.ctx.scopes.exit_scope();
        body_ty
    }

    fn check_match_range_value_pattern(&mut self, value: &Expr, target_ty: TypeId) -> bool {
        match &value.kind {
            ExprKind::Grouped { expr, .. } => self.check_match_range_value_pattern(expr, target_ty),
            ExprKind::Range {
                start: Some(start),
                end: Some(end),
                ..
            } => {
                let start_ty = self.check_expr(start, Some(target_ty));
                self.check_coercion(start, target_ty, start_ty);
                let end_ty = self.check_expr(end, Some(target_ty));
                self.check_coercion(end, target_ty, end_ty);
                true
            }
            ExprKind::Range { .. } => {
                self.ctx
                    .struct_error(
                        value.span,
                        "open-ended range patterns are not supported here",
                    )
                    .with_hint("use a closed scalar range such as `start...end` or `start..=end`")
                    .emit();
                true
            }
            _ => false,
        }
    }

    pub(crate) fn check_return(&mut self, val: Option<&Expr>, span: Span) -> TypeId {
        self.has_returned = true;
        let expected_ret = self.current_return_type.unwrap_or(TypeId::VOID);

        if let Some(v) = val {
            // Thread the function's expected return type into the returned expression.
            let val_ty = self.check_expr(v, Some(expected_ret));

            if let Some(ret_ty) = self.current_return_type
                && !self.reject_returned_capturing_closure(v, ret_ty, val_ty)
            {
                self.check_coercion(v, ret_ty, val_ty);
            }
            self.reject_stack_pointer_escape(v, "a return value");
        } else if expected_ret != TypeId::VOID && expected_ret != TypeId::ERROR {
            let ret_str = self.ctx.ty_to_string(expected_ret);
            self.ctx
                .struct_error(span, "expected a return value, but found empty return")
                .with_hint(format!("function is expected to return `{}`", ret_str))
                .emit();
        }
        TypeId::VOID
    }

    pub(crate) fn check_while(&mut self, cond: &Expr, body: &Expr) -> TypeId {
        self.ctx.scopes.enter_scope();
        let c_ty = self.check_expr(cond, Some(TypeId::BOOL));
        self.check_coercion(cond, TypeId::BOOL, c_ty);
        let _ = self.check_discarded_expr(body);
        self.ctx.scopes.exit_scope();
        TypeId::VOID
    }

    /// Check whether a standalone expression illegally discards a non-void value.
    fn check_discarded_expr(&mut self, expr: &Expr) -> TypeId {
        let ty = self.check_expr(expr, None);
        let norm_ty = self.resolve_tv(ty);

        // Only `void`, `never`, or already-invalid expressions may be dropped implicitly.
        if norm_ty != TypeId::VOID && norm_ty != TypeId::NEVER && norm_ty != TypeId::ERROR {
            let ty_str = self.ctx.ty_to_string(ty);
            self.ctx
                .struct_error(expr.span, "ignored non-void return value")
                .with_code(DiagnosticCode::IgnoredNonvoidValue)
                .with_hint(format!(
                    "expression evaluates to `{}`, which must be explicitly used or discarded",
                    ty_str
                ))
                .with_hint("in Kern, use `_ = ...;` to explicitly discard the value")
                .emit();
        }
        ty
    }

    pub(crate) fn check_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let outer_scope = self.ctx.scopes.current_scope_id();
        let mut entered_scope = false;
        let mut saw_diverging_stmt = false;
        for stmt in stmts {
            if self.is_canceled() {
                if entered_scope {
                    if let Some(scope_id) = outer_scope {
                        self.ctx.scopes.set_current_scope(scope_id);
                    } else {
                        self.ctx.scopes.exit_scope();
                    }
                }
                return TypeId::ERROR;
            }
            match &stmt.kind {
                StmtKind::Use(use_stmt) => {
                    let import = ImportDef {
                        path_kind: use_stmt.kind,
                        path: use_stmt.path.clone(),
                        target: use_stmt.target.clone(),
                        vis: ast::Visibility::Private,
                        span: stmt.span,
                        binding_span: use_stmt.binding_span,
                    };

                    if self.import_needs_scope_extension(&import, entered_scope) {
                        entered_scope = true;
                        self.ctx.scopes.enter_scope();
                    }

                    let Some(current_scope) = self.ctx.scopes.current_scope_id() else {
                        self.ctx.emit_ice(
                            stmt.span,
                            "Kern ICE (Typeck): missing active scope while resolving a local import",
                        );
                        continue;
                    };
                    let Some(current_module) = self.ctx.module_for_scope(current_scope) else {
                        self.ctx.emit_ice(
                            stmt.span,
                            "Kern ICE (Typeck): could not determine module for a local import",
                        );
                        continue;
                    };

                    {
                        let mut resolver = ImportResolver::new(self.ctx);
                        let _ = resolver.resolve_import_into_scope(
                            current_module,
                            current_scope,
                            &import,
                            true,
                        );
                    }
                }
                StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                    let needs_scope_extension = match &e.kind {
                        ExprKind::Let { pattern, .. } => {
                            self.let_pattern_needs_scope_extension(pattern, entered_scope)
                        }
                        ExprKind::Static { pattern, .. } => {
                            self.binding_pattern_needs_scope_extension(pattern, entered_scope)
                        }
                        _ => false,
                    };
                    if needs_scope_extension {
                        // The first binding creates the block-local environment. Subsequent
                        // bindings only need a fresh child scope when they shadow a visible name.
                        entered_scope = true;
                        self.ctx.scopes.enter_scope();
                    }
                    let stmt_ty = self.check_discarded_expr(e);
                    if self.resolve_tv(stmt_ty) == TypeId::NEVER {
                        saw_diverging_stmt = true;
                    }
                }
            }
        }
        let ret_ty = if saw_diverging_stmt {
            if let Some(res) = result {
                let _ = self.check_expr(res, expected_ty);
            }
            TypeId::NEVER
        } else if let Some(res) = result {
            self.check_expr(res, expected_ty)
        } else {
            TypeId::VOID
        };
        if entered_scope {
            if let Some(scope_id) = outer_scope {
                self.ctx.scopes.set_current_scope(scope_id);
            } else {
                self.ctx.scopes.exit_scope();
            }
        } else if let Some(scope_id) = outer_scope {
            self.ctx.scopes.set_current_scope(scope_id);
        }
        ret_ty
    }

    pub(crate) fn check_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let cond_ty = self.check_expr(cond, Some(TypeId::BOOL));
        self.check_coercion(cond, TypeId::BOOL, cond_ty);

        let then_ty = self.check_expr(then_branch, expected_ty);
        if let Some(else_expr) = else_branch {
            let else_ty = self.check_expr(else_expr, expected_ty);

            // If one branch diverges, use the other branch's type.
            if then_ty == TypeId::NEVER {
                return else_ty;
            } else if else_ty == TypeId::NEVER {
                return then_ty;
            }

            self.check_coercion(else_expr, then_ty, else_ty);
            then_ty
        } else {
            TypeId::VOID
        }
    }

    pub(crate) fn check_defer(&mut self, defer_expr: &Expr) -> TypeId {
        let _ = self.check_discarded_expr(defer_expr);
        TypeId::VOID
    }
}
