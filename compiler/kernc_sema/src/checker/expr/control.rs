use super::ExprChecker;
use crate::LayoutEngine;
use crate::checker::{ConstEvaluator, ConstValue};
use crate::def::{Def, ImportDef};
use crate::passes::ImportResolver;
use crate::ty::{PrimitiveType, TypeId, TypeKind};
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
                    let Some(next_cursor) = interval.end.checked_add(1) else {
                        return None;
                    };
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
                    let Some(next_cursor) = interval.end.checked_add(1) else {
                        return None;
                    };
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
                    let field_ty = self
                        .ctx
                        .facts
                        .node_types
                        .get(&field.type_node.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
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
                // Safety: semantic defs are immutable while type checking expressions.
                let adt_def = unsafe { &*adt_def }.clone();
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
                                    let ty = self
                                        .ctx
                                        .facts
                                        .node_types
                                        .get(&payload.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR);
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
                    unreachable!("struct constructor expected for struct coverage lowering");
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
            ExprKind::Bool(value) if norm_target == TypeId::BOOL => Some(
                CoveragePattern::Constructor(CoverageConstructorKind::Bool(*value), Vec::new()),
            ),
            ExprKind::EnumLiteral { variant, .. } => Some(CoveragePattern::Constructor(
                CoverageConstructorKind::EnumVariant(*variant),
                Vec::new(),
            )),
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
                    unreachable!("struct constructor expected for struct value-pattern coverage");
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
            ast::MatchPatternKind::Range { .. } => None,
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
            ast::MatchPatternKind::Range {
                start,
                end,
                inclusive,
            } => {
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
                        let end = if *inclusive {
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
                        let end = if *inclusive {
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
            ast::MatchPatternKind::Pattern(_) => None,
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
            let body_ty = self.check_match_arm(
                arm,
                norm_target,
                has_constructor_coverage,
                common_ret_ty,
                &mut seen_patterns,
                scalar_coverage.as_mut(),
                &mut match_closed,
                &mut has_catch_all,
            );

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
        norm_target: TypeId,
        has_constructor_coverage: bool,
        common_ret_ty: Option<TypeId>,
        seen_patterns: &mut Vec<Vec<CoveragePattern>>,
        scalar_coverage: Option<&mut ScalarCoverageState>,
        match_closed: &mut bool,
        has_catch_all: &mut bool,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        let mut scalar_coverage = scalar_coverage;

        let pattern_started = self.timing_start();
        for pat in &arm.patterns {
            match &pat.kind {
                ast::MatchPatternKind::Value(v) => {
                    let v_ty = self.check_expr(v, Some(norm_target));
                    self.check_coercion(v, norm_target, v_ty);
                    if *match_closed {
                        self.warn_unreachable_match_pattern(pat.span);
                    } else if has_constructor_coverage
                        && let Some(lowered) = self.coverage_lower_match_pattern(pat, norm_target)
                    {
                        if self.coverage_vector_is_useful(
                            &[norm_target],
                            seen_patterns,
                            std::slice::from_ref(&lowered),
                        ) {
                            seen_patterns.push(vec![lowered]);
                            if self.coverage_matrix_is_exhaustive(norm_target, seen_patterns) {
                                *has_catch_all = true;
                                *match_closed = true;
                            }
                        } else {
                            self.warn_unreachable_match_pattern(pat.span);
                        }
                    } else if let Some(scalar_coverage) = scalar_coverage.as_deref_mut()
                        && let Some(intervals) =
                            self.scalar_pattern_intervals(pat, norm_target, scalar_coverage)
                    {
                        if intervals.is_empty() || scalar_coverage.covers_all(&intervals) {
                            self.warn_unreachable_match_pattern(pat.span);
                        } else {
                            scalar_coverage.add_intervals(&intervals);
                            if scalar_coverage.is_full() {
                                *has_catch_all = true;
                                *match_closed = true;
                            }
                        }
                    }
                }
                ast::MatchPatternKind::Range { start, end, .. } => {
                    let s_ty = self.check_expr(start, Some(norm_target));
                    let e_ty = self.check_expr(end, Some(norm_target));
                    self.check_coercion(start, norm_target, s_ty);
                    self.check_coercion(end, norm_target, e_ty);
                    if *match_closed {
                        self.warn_unreachable_match_pattern(pat.span);
                    } else if let Some(scalar_coverage) = scalar_coverage.as_deref_mut()
                        && let Some(intervals) =
                            self.scalar_pattern_intervals(pat, norm_target, scalar_coverage)
                    {
                        if intervals.is_empty() || scalar_coverage.covers_all(&intervals) {
                            self.warn_unreachable_match_pattern(pat.span);
                        } else {
                            scalar_coverage.add_intervals(&intervals);
                            if scalar_coverage.is_full() {
                                *has_catch_all = true;
                                *match_closed = true;
                            }
                        }
                    }
                }
                ast::MatchPatternKind::Pattern(pattern) => {
                    self.check_pattern(arm.body.id, pattern, norm_target);

                    let irrefutable = self.pattern_is_irrefutable(pattern, norm_target);
                    if *match_closed {
                        self.warn_unreachable_match_pattern(pat.span);
                    } else if has_constructor_coverage
                        && let Some(lowered) = self.coverage_lower_match_pattern(pat, norm_target)
                    {
                        if self.coverage_vector_is_useful(
                            &[norm_target],
                            seen_patterns,
                            std::slice::from_ref(&lowered),
                        ) {
                            seen_patterns.push(vec![lowered]);
                            if irrefutable
                                || self.coverage_matrix_is_exhaustive(norm_target, seen_patterns)
                            {
                                *has_catch_all = true;
                                *match_closed = true;
                            }
                        } else {
                            self.warn_unreachable_match_pattern(pat.span);
                        }
                    } else if let Some(scalar_coverage) = scalar_coverage.as_deref_mut() {
                        if irrefutable {
                            if scalar_coverage.is_full() {
                                self.warn_unreachable_match_pattern(pat.span);
                            } else {
                                *has_catch_all = true;
                                *match_closed = true;
                            }
                        }
                    } else if irrefutable {
                        *has_catch_all = true;
                        *match_closed = true;
                    }
                }
            }
        }
        self.record_expr_timing(pattern_started, |stats, elapsed| {
            stats.control_match_patterns += elapsed;
        });

        let body_started = self.timing_start();
        let body_ty = self.check_expr(&arm.body, common_ret_ty);
        self.record_expr_timing(body_started, |stats, elapsed| {
            stats.control_match_bodies += elapsed;
        });
        self.ctx.scopes.exit_scope();
        body_ty
    }

    pub(crate) fn check_return(&mut self, val: Option<&Expr>, span: Span) -> TypeId {
        self.has_returned = true;
        let expected_ret = self.current_return_type.unwrap_or(TypeId::VOID);

        if let Some(v) = val {
            // Thread the function's expected return type into the returned expression.
            let val_ty = self.check_expr(v, Some(expected_ret));

            if let Some(ret_ty) = self.current_return_type {
                if !self.reject_returned_capturing_closure(v, ret_ty, val_ty) {
                    self.check_coercion(v, ret_ty, val_ty);
                }
            }
        } else if expected_ret != TypeId::VOID && expected_ret != TypeId::ERROR {
            let ret_str = self.ctx.ty_to_string(expected_ret);
            self.ctx
                .struct_error(span, "expected a return value, but found empty return")
                .with_hint(format!("function is expected to return `{}`", ret_str))
                .emit();
        }
        TypeId::VOID
    }

    pub(crate) fn check_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        if let Some(i) = init {
            let _ = self.check_expr(i, None);
        }
        if let Some(c) = cond {
            let c_ty = self.check_expr(c, Some(TypeId::BOOL));
            self.check_coercion(c, TypeId::BOOL, c_ty);
        }
        if let Some(p) = post {
            let _ = self.check_expr(p, None);
        }
        let _ = self.check_expr(body, None);
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
                .with_hint("in Kern, use `let _ = ...;` to explicitly discard the value")
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
