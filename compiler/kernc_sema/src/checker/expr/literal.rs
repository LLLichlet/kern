use super::{ExprChecker, NumericInferenceKind};
use crate::checker::ConstEvaluator;
use crate::def::Def;
use crate::passes::TypeResolver;
use crate::ty::{
    ConstGeneric, ConstGenericValue, ConstGenericValueKind, GenericArg, Substituter, TypeId,
    TypeKind,
};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

type StructLiteralDefInfo = (
    Vec<StructLiteralFieldInfo>,
    String,
    Vec<ast::GenericParam>,
    Vec<GenericArg>,
    bool,
);

#[derive(Clone, Copy)]
struct StructLiteralFieldInfo {
    name: kernc_utils::SymbolId,
    ty: TypeId,
    has_default: bool,
    definition_span: Option<Span>,
    vis: Option<ast::Visibility>,
    owner_def: Option<crate::def::DefId>,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn data_field_init_is_pun(&self, field: &ast::StructFieldInit) -> bool {
        matches!(
            &field.value.kind,
            ExprKind::Identifier(name)
                if *name == field.name && field.value.span == field.name_span
        )
    }

    fn data_field_inits_are_puns(&self, fields: &[ast::StructFieldInit]) -> bool {
        fields
            .iter()
            .all(|field| self.data_field_init_is_pun(field))
    }

    fn data_array_elems_as_field_puns(&self, elems: &[Expr]) -> Option<Vec<ast::StructFieldInit>> {
        elems
            .iter()
            .map(|elem| {
                let ExprKind::Identifier(name) = elem.kind else {
                    return None;
                };
                Some(ast::StructFieldInit {
                    name,
                    name_span: elem.span,
                    value: elem.clone(),
                    span: elem.span,
                })
            })
            .collect()
    }

    fn data_expr_as_field_pun(&self, expr: &Expr) -> Option<ast::StructFieldInit> {
        let ExprKind::Identifier(name) = expr.kind else {
            return None;
        };
        Some(ast::StructFieldInit {
            name,
            name_span: expr.span,
            value: expr.clone(),
            span: expr.span,
        })
    }

    fn data_literal_target_is_array_like(&self, kind: &TypeKind) -> bool {
        matches!(
            kind,
            TypeKind::Array { .. }
                | TypeKind::ArrayInfer { .. }
                | TypeKind::Slice { .. }
                | TypeKind::Simd { .. }
        )
    }

    fn data_literal_target_is_structural(&self, kind: &TypeKind) -> bool {
        match kind {
            TypeKind::Def(def_id, _) => matches!(
                self.ctx.defs.get(def_id.0 as usize),
                Some(Def::Struct(_)) | Some(Def::Union(_))
            ),
            TypeKind::AnonymousStruct(_, _) | TypeKind::AnonymousUnion(_, _) => true,
            _ => false,
        }
    }

    pub(crate) fn resolve_data_init_target_type(
        &mut self,
        type_node: Option<&ast::TypeNode>,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        if let Some(ty_ast) = type_node {
            let mut resolver = TypeResolver::new(self.ctx);
            let Some(scope) = resolver.current_scope_id() else {
                resolver.context().emit_ice(
                    span,
                    "Compiler ICE: explicit data literal type was resolved without an active scope.",
                );
                return TypeId::ERROR;
            };
            return resolver.resolve_type(ty_ast, scope);
        }

        if let Some(exp) = expected_ty {
            return self.resolve_tv(exp);
        }

        self.ctx
            .struct_error(
                span,
                "cannot infer type for anonymous initialization `.{...}`",
            )
            .with_hint(
                "provide an explicit type context or prepend the type name, e.g., `MyStruct.{...}`",
            )
            .emit();
        TypeId::ERROR
    }

    fn check_anon_enum_payload_literal(
        &mut self,
        init_fields: &[ast::StructFieldInit],
        expected: TypeId,
        enum_def: &crate::ty::AnonymousEnum,
        span: Span,
    ) -> TypeId {
        if init_fields.len() != 1 {
            self.ctx
                .struct_error(span, "Enum literal must specify exactly one variant")
                .emit();
            return TypeId::ERROR;
        }

        let init_f = &init_fields[0];
        let variant = enum_def.variants.iter().find(|v| v.name == init_f.name);
        if let Some(v) = variant {
            let definition_span = v.name_span;
            let payload_ty = v.payload_ty;
            self.ctx
                .record_identifier_reference(init_f.name_span, definition_span);
            if let Some(payload_ty) = payload_ty {
                let val_ty = self.check_expr(&init_f.value, Some(payload_ty));
                self.check_coercion(&init_f.value, payload_ty, val_ty);
            } else {
                let v_str = self.ctx.resolve(v.name).to_string();
                let expected_str = self.ctx.ty_to_string(expected);
                self.ctx
                    .struct_error(
                        init_f.span,
                        format!("variant `{}` does not take a payload", v_str),
                    )
                    .with_hint(format!(
                        "use direct variant syntax like `.{}` or `{}.{}`",
                        v_str, expected_str, v_str
                    ))
                    .emit();
            }
        } else {
            let v_str = self.ctx.resolve(init_f.name);
            self.ctx
                .struct_error(
                    init_f.span,
                    format!("variant `{}` not found in anonymous enum", v_str),
                )
                .emit();
        }

        expected
    }

    fn numeric_literal_suffix_type(&self, suffix: ast::NumericLiteralSuffix) -> TypeId {
        match suffix {
            ast::NumericLiteralSuffix::I8 => TypeId::I8,
            ast::NumericLiteralSuffix::I16 => TypeId::I16,
            ast::NumericLiteralSuffix::I32 => TypeId::I32,
            ast::NumericLiteralSuffix::I64 => TypeId::I64,
            ast::NumericLiteralSuffix::I128 => TypeId::I128,
            ast::NumericLiteralSuffix::ISize => TypeId::ISIZE,
            ast::NumericLiteralSuffix::U8 => TypeId::U8,
            ast::NumericLiteralSuffix::U16 => TypeId::U16,
            ast::NumericLiteralSuffix::U32 => TypeId::U32,
            ast::NumericLiteralSuffix::U64 => TypeId::U64,
            ast::NumericLiteralSuffix::U128 => TypeId::U128,
            ast::NumericLiteralSuffix::USize => TypeId::USIZE,
            ast::NumericLiteralSuffix::F32 => TypeId::F32,
            ast::NumericLiteralSuffix::F64 => TypeId::F64,
        }
    }

    pub(crate) fn check_integer(
        &mut self,
        _expr: &Expr,
        suffix: Option<ast::NumericLiteralSuffix>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        if let Some(suffix) = suffix {
            let ty = self.numeric_literal_suffix_type(suffix);
            if !self.ctx.type_registry.is_integer(ty) {
                self.ctx
                    .struct_error(
                        _expr.span,
                        "integer literal cannot use a floating-point suffix",
                    )
                    .with_hint("use an integer suffix such as `i32`, `u64`, or `usize`")
                    .emit();
                return TypeId::ERROR;
            }
            return ty;
        }

        if let Some(exp) = expected_ty {
            let norm = self.resolve_tv(exp);

            // Reuse the expected numeric type when the context already constrained it.
            if self.ctx.type_registry.is_integer(norm)
                || self.ctx.type_registry.is_float(norm)
                || self.type_numeric_candidates(norm).is_some()
                || self.expected_type_satisfies_builtin_marker(exp, "Integer")
                || self.expected_type_satisfies_builtin_marker(exp, "SignedInteger")
                || self.expected_type_satisfies_builtin_marker(exp, "UnsignedInteger")
                || self.expected_type_satisfies_builtin_marker(exp, "Float")
            {
                return exp;
            }

            return TypeId::I32;
        }

        self.fresh_numeric_type_var(NumericInferenceKind::IntLiteral)
    }

    pub(crate) fn check_float(
        &mut self,
        _expr: &Expr,
        suffix: Option<ast::NumericLiteralSuffix>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        if let Some(suffix) = suffix {
            let ty = self.numeric_literal_suffix_type(suffix);
            if !self.ctx.type_registry.is_float(ty) {
                self.ctx
                    .struct_error(
                        _expr.span,
                        "floating-point literal cannot use an integer suffix",
                    )
                    .with_hint("use a floating-point suffix such as `f32` or `f64`")
                    .emit();
                return TypeId::ERROR;
            }
            return ty;
        }

        if let Some(exp) = expected_ty {
            let norm = self.resolve_tv(exp);
            // Reuse the expected float type when available.
            if self.ctx.type_registry.is_float(norm)
                || self
                    .type_numeric_candidates(norm)
                    .is_some_and(Self::numeric_candidates_have_floats)
                || self.expected_type_satisfies_builtin_marker(exp, "Float")
            {
                return exp;
            }

            return TypeId::F64;
        }

        self.fresh_numeric_type_var(NumericInferenceKind::FloatLiteral)
    }

    fn expected_type_satisfies_builtin_marker(&mut self, ty: TypeId, marker_name: &str) -> bool {
        let Some(marker_id) = self.ctx.builtin_def(marker_name) else {
            return false;
        };
        let marker_ty =
            self.ctx
                .type_registry
                .intern(TypeKind::TraitObject(marker_id, Vec::new(), Vec::new()));
        self.check_trait_impl(ty, marker_ty)
    }

    pub(crate) fn check_data_init_expr(
        &mut self,
        target_ty: TypeId,
        literal: &ast::DataLiteralKind,
        span: Span,
    ) -> TypeId {
        if target_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        self.check_data_literal(literal, target_ty, span)
    }

    fn check_data_literal(
        &mut self,
        kind: &ast::DataLiteralKind,
        expected: TypeId,
        span: Span,
    ) -> TypeId {
        if matches!(kind, ast::DataLiteralKind::Scalar(inner) if matches!(inner.kind, ExprKind::Undef))
        {
            self.ctx
                .struct_error(span, "`undef` is not a data initializer")
                .with_hint("write `let value: Type = undef` or use `field: undef` where an expected type is available")
                .emit();
            return TypeId::ERROR;
        }

        let exp_norm = self.resolve_tv(expected);
        let kind_enum = self.ctx.type_registry.get(exp_norm).clone();

        if let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } = kind_enum {
            self.ctx
                .struct_error(span, "pointer types cannot be initialized with `.{...}`")
                .with_hint(
                    "use `.&` or `..&` to take an address, or use `as` for an explicit pointer conversion",
                )
                .with_hint("use an expected type context for natural trait-object or closure-object packaging")
                .emit();
            return TypeId::ERROR;
        }

        // Handle `void.{}` specially.
        if self.ctx.type_registry.is_void(exp_norm) {
            match kind {
                // Empty array and empty struct fallbacks are both valid here.
                ast::DataLiteralKind::Array(elems) if elems.is_empty() => return expected,
                ast::DataLiteralKind::Struct(fields) if fields.is_empty() => return expected,
                _ => {
                    self.ctx.struct_error(span, "`void` is a zero-sized type and can only be initialized with empty braces `.{}`").emit();
                    return TypeId::ERROR;
                }
            }
        }

        // Normalize enum detection across named and anonymous forms.
        let is_data = matches!(kind_enum, TypeKind::Enum(..) | TypeKind::AnonymousEnum(..));

        match kind {
            ast::DataLiteralKind::Array(elems) => {
                let is_target_array_like = matches!(
                    kind_enum,
                    TypeKind::Array { .. }
                        | TypeKind::ArrayInfer { .. }
                        | TypeKind::Slice { .. }
                        | TypeKind::Simd { .. }
                );
                if elems.is_empty() && !is_target_array_like {
                    if is_data {
                        self.check_enum_payload_literal(&[], expected, exp_norm, span)
                    } else {
                        self.check_struct_or_union_literal(&[], expected, exp_norm, span)
                    }
                } else if !is_target_array_like
                    && self.data_literal_target_is_structural(&kind_enum)
                {
                    if let Some(init_fields) = self.data_array_elems_as_field_puns(elems) {
                        self.check_struct_or_union_literal(&init_fields, expected, exp_norm, span)
                    } else {
                        self.check_array_literal(elems, expected, exp_norm, span)
                    }
                } else {
                    self.check_array_literal(elems, expected, exp_norm, span)
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.check_repeat_literal(value, count, expected, exp_norm, span)
            }
            ast::DataLiteralKind::Struct(init_fields) => {
                if is_data {
                    self.check_enum_payload_literal(init_fields, expected, exp_norm, span)
                } else if self.data_literal_target_is_structural(&kind_enum) {
                    self.check_struct_or_union_literal(init_fields, expected, exp_norm, span)
                } else if self.data_field_inits_are_puns(init_fields) {
                    let elems = init_fields
                        .iter()
                        .map(|field| field.value.clone())
                        .collect::<Vec<_>>();
                    if self.data_literal_target_is_array_like(&kind_enum) {
                        self.check_array_literal(&elems, expected, exp_norm, span)
                    } else if let [inner] = elems.as_slice() {
                        self.check_scalar_literal(inner, expected)
                    } else {
                        self.check_array_literal(&elems, expected, exp_norm, span)
                    }
                } else {
                    self.check_struct_or_union_literal(init_fields, expected, exp_norm, span)
                }
            }
            ast::DataLiteralKind::Scalar(inner) => {
                let is_target_array_like = self.data_literal_target_is_array_like(&kind_enum);
                if is_target_array_like && !matches!(inner.kind, ExprKind::Undef) {
                    self.check_array_literal(std::slice::from_ref(inner), expected, exp_norm, span)
                } else if self.data_literal_target_is_structural(&kind_enum) {
                    if let Some(init_field) = self.data_expr_as_field_pun(inner) {
                        self.check_struct_or_union_literal(&[init_field], expected, exp_norm, span)
                    } else {
                        self.check_scalar_literal(inner, expected)
                    }
                } else if is_data {
                    if let ExprKind::Identifier(variant_name) = &inner.kind {
                        let variant = self.ctx.resolve(*variant_name).to_string();
                        let expected_name = self.ctx.ty_to_string(expected);
                        self.ctx
                            .struct_error(
                                inner.span,
                                format!(
                                    "payload-less enum variants must use direct variant syntax, not `{{ {} }}`",
                                    variant
                                ),
                            )
                            .with_hint(format!(
                                "write `.{}` or `{}.{}` instead",
                                variant, expected_name, variant
                            ))
                            .emit();
                    } else {
                        self.ctx
                            .struct_error(
                                inner.span,
                                "enum initialization inside `{ ... }` requires a payload field like `{ Variant: value }`",
                            )
                            .with_hint("payload-less variants must be written as `.Variant` or `Type.Variant`")
                            .emit();
                    }
                    TypeId::ERROR
                } else {
                    self.check_scalar_literal(inner, expected)
                }
            }
        }
    }

    /// Validate `.Variant` shorthand and direct `Type.Variant` payload-less enum construction.
    pub(crate) fn check_enum_literal(
        &mut self,
        variant_name: SymbolId,
        variant_span: Span,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let mut res_ty = TypeId::ERROR;
        if let Some(exp_ty) = expected_ty {
            let norm_exp = self.resolve_tv(exp_ty);
            if let TypeKind::Enum(def_id, _) = self.ctx.type_registry.get(norm_exp) {
                if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
                    if let Some(v) = d.variants.iter().find(|v| v.name == variant_name) {
                        let definition_span = v.name_span;
                        let requires_payload = v.payload_type.is_some();
                        self.ctx
                            .record_identifier_reference(variant_span, definition_span);
                        // Payload-carrying variants must use structured initialization.
                        if requires_payload {
                            let v_str = self.ctx.resolve(variant_name).to_string();
                            self.ctx
                                .struct_error(
                                    span,
                                    format!("variant `{}` requires a payload", v_str),
                                )
                                .with_hint(format!("initialize it as `.{{ {}: value }}`", v_str))
                                .emit();
                        } else {
                            res_ty = exp_ty;
                        }
                    } else {
                        let v_str = self.ctx.resolve(variant_name).to_string();
                        let exp_str = self.ctx.ty_to_string(norm_exp);
                        let available_variants: Vec<String> = d
                            .variants
                            .iter()
                            .map(|v| format!(".{}", self.ctx.resolve(v.name)))
                            .collect();
                        let mut diag = self
                            .ctx
                            .struct_error(
                                span,
                                format!(
                                    "variant `.{}` does not exist in the expected data type",
                                    v_str
                                ),
                            )
                            .with_hint(format!("expected data type is `{}`", exp_str));

                        if !available_variants.is_empty() {
                            diag = diag.with_hint(format!(
                                "available variants: {}",
                                available_variants.join(", ")
                            ));
                        }
                        diag.emit();
                    }
                }
            } else if let TypeKind::AnonymousEnum(enum_def) = self.ctx.type_registry.get(norm_exp) {
                if let Some(v) = enum_def.variants.iter().find(|v| v.name == variant_name) {
                    let definition_span = v.name_span;
                    let requires_payload = v.payload_ty.is_some();
                    self.ctx
                        .record_identifier_reference(variant_span, definition_span);
                    if requires_payload {
                        let v_str = self.ctx.resolve(variant_name).to_string();
                        self.ctx
                            .struct_error(span, format!("variant `{}` requires a payload", v_str))
                            .with_hint(format!("initialize it as `.{{ {}: value }}`", v_str))
                            .emit();
                    } else {
                        res_ty = exp_ty;
                    }
                } else {
                    let v_str = self.ctx.resolve(variant_name).to_string();
                    let exp_str = self.ctx.ty_to_string(norm_exp);
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "variant `.{}` does not exist in the expected data type",
                                v_str
                            ),
                        )
                        .with_hint(format!("expected data type is `{}`", exp_str))
                        .emit();
                }
            } else if norm_exp != TypeId::ERROR {
                let exp_str = self.ctx.ty_to_string(norm_exp);
                self.ctx
                    .struct_error(span, "expected a data/enum type for variant literal")
                    .with_hint(format!("but context expects `{}`", exp_str))
                    .emit();
            }
        } else {
            self.ctx
                .struct_error(
                    span,
                    "cannot infer data type for variant literal without context",
                )
                .with_hint("try prepending the type name, e.g., `Result.Ok` instead of `.Ok`")
                .emit();
        }
        res_ty
    }

    /// Handle payload-carrying enum initialization such as `Result.{ Ok: 10 }`.
    fn check_enum_payload_literal(
        &mut self,
        init_fields: &[ast::StructFieldInit],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        let anon_enum =
            if let TypeKind::AnonymousEnum(enum_def) = self.ctx.type_registry.get(exp_norm) {
                Some(enum_def.clone())
            } else {
                None
            };
        if let Some(enum_def) = anon_enum.as_ref() {
            return self.check_anon_enum_payload_literal(init_fields, expected, enum_def, span);
        }

        let (def_id, generic_args) = match self.ctx.type_registry.get(exp_norm) {
            TypeKind::Enum(id, args) => (*id, args.clone()),
            _ => {
                let exp_str = self.ctx.ty_to_string(exp_norm);
                self.ctx
                    .struct_error(span, "expected an enum type for enum payload literal")
                    .with_hint(format!("context expects `{}`", exp_str))
                    .emit();
                return TypeId::ERROR;
            }
        };

        let data_def = match &self.ctx.defs[def_id.0 as usize] {
            Def::Enum(d) => d.clone(),
            _ => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: enum payload literal resolved to non-enum DefId {}.",
                        def_id.0
                    ),
                );
                return TypeId::ERROR;
            }
        };

        if init_fields.len() != 1 {
            self.ctx
                .struct_error(span, "Enum literal must specify exactly one variant")
                .emit();
            return TypeId::ERROR;
        }

        let init_f = &init_fields[0];
        let variant = data_def.variants.iter().find(|v| v.name == init_f.name);

        if let Some(v) = variant {
            let definition_span = v.name_span;
            let payload_ast = v.payload_type.clone();
            self.ctx
                .record_identifier_reference(init_f.name_span, definition_span);
            if let Some(payload_ast) = &payload_ast {
                let mut payload_ty = self.ctx.node_type_or_error(payload_ast.id);

                if !data_def.generics.is_empty() && !generic_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in data_def.generics.iter().enumerate() {
                        map.insert(param.name, generic_args[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    payload_ty = subst.substitute(payload_ty);
                }

                let val_ty = self.check_expr(&init_f.value, Some(payload_ty));
                self.check_coercion(&init_f.value, payload_ty, val_ty);
            } else {
                let v_str = self.ctx.resolve(v.name).to_string();
                let expected_str = self.ctx.ty_to_string(expected);
                self.ctx
                    .struct_error(
                        init_f.span,
                        format!("variant `{}` does not take a payload", v_str),
                    )
                    .with_hint(format!(
                        "use direct variant syntax like `.{}` or `{}.{}`",
                        v_str, expected_str, v_str
                    ))
                    .emit();
            }
        } else {
            let v_str = self.ctx.resolve(init_f.name);
            let data_str = self.ctx.resolve(data_def.name);
            self.ctx
                .struct_error(
                    init_f.span,
                    format!("variant `{}` not found in data type `{}`", v_str, data_str),
                )
                .emit();
        }

        expected
    }

    /// Helper 1: validate standard array literals like `.{ 1, 2, 3 }`.
    fn check_array_literal(
        &mut self,
        elems: &[Expr],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. Peel aliases and wrappers to inspect the real container type.
        let (exp_elem_ty, expected_len, preserve_slice_ty) =
            match self.ctx.type_registry.get(exp_norm) {
                TypeKind::Array { elem, len } => (*elem, Some(*len), false),
                TypeKind::ArrayInfer { elem } => (*elem, None, false),
                // Explicit `&[T].{ ... }` stays slice-typed. Only contextual array inference
                // synthesizes a fresh `[N]T`.
                TypeKind::Slice { elem, .. } => (*elem, None, true),
                TypeKind::Simd { elem, lanes } => (
                    *elem,
                    Some(ConstGeneric::Value(ConstGenericValue {
                        ty: TypeId::USIZE,
                        kind: ConstGenericValueKind::Int(*lanes as i128),
                    })),
                    false,
                ),
                _ => {
                    let ty_str = self.ctx.ty_to_string(expected);
                    self.ctx
                        .struct_error(
                            span,
                            "expected an array, slice, or SIMD type for literal `.{ ... }`",
                        )
                        .with_hint(format!("context expects `{}`", ty_str))
                        .emit();
                    return TypeId::ERROR;
                }
            };

        // 2. Check the length when the target is a fixed-size array.
        if let Some(ConstGeneric::Value(len)) = expected_len
            && len.ty == TypeId::USIZE
            && Some(elems.len() as i128) != len.as_int()
        {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "array literal length ({}) does not match expected length ({})",
                        elems.len(),
                        len.as_int().unwrap_or_default()
                    ),
                )
                .emit();
        }

        // 3. Check the type of every element.
        for e in elems {
            if self.is_canceled() {
                return TypeId::ERROR;
            }
            let act_ty = self.check_expr(e, Some(exp_elem_ty));
            self.check_coercion(e, exp_elem_ty, act_ty);
        }

        // 4. Return the final inferred array type.
        if preserve_slice_ty {
            expected
        } else if expected_len.is_none() {
            let actual_len = elems.len() as u64;
            if actual_len > u32::MAX as u64 {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "array length {} exceeds the current compiler limit of {} elements",
                            actual_len,
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
                elem: exp_elem_ty,
                len: ConstGeneric::Value(ConstGenericValue {
                    ty: TypeId::USIZE,
                    kind: ConstGenericValueKind::Int(actual_len as i128),
                }),
            })
        } else {
            // Already a concrete `[N]T`.
            expected
        }
    }

    /// Helper 2: validate repeated array literals like `.{ 0; 1024 }`.
    fn check_repeat_literal(
        &mut self,
        value: &Expr,
        count: &Expr,
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. Peel aliases and wrappers to inspect the real container type.
        let simd_info = self.ctx.type_registry.simd_info(exp_norm);
        let (exp_elem_ty, is_infer, preserve_slice_ty) = match self.ctx.type_registry.get(exp_norm)
        {
            TypeKind::Array { elem, .. } => (*elem, false, false),
            TypeKind::ArrayInfer { elem } => (*elem, true, false),
            TypeKind::Slice { elem, .. } => (*elem, true, true),
            TypeKind::Simd { .. } => {
                let mut ce = ConstEvaluator::new(self.ctx);
                let Ok(actual_len) = ce.eval_usize(count) else {
                    return TypeId::ERROR;
                };
                let Some((elem, lanes)) = simd_info else {
                    return TypeId::ERROR;
                };
                if actual_len != lanes as u64 {
                    self.ctx
                        .struct_error(
                            count.span,
                            format!(
                                "repeat literal count ({}) does not match SIMD lane count ({})",
                                actual_len, lanes
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                (elem, false, false)
            }
            _ => {
                let ty_str = self.ctx.ty_to_string(expected);
                self.ctx
                    .struct_error(
                        span,
                        "expected an array, slice, or SIMD type for repeat literal `.{ v; N }`",
                    )
                    .with_hint(format!("context expects `{}`", ty_str))
                    .emit();
                return TypeId::ERROR;
            }
        };

        // 2. Check the repeated element value.
        let val_ty = self.check_expr(value, Some(exp_elem_ty));
        self.check_coercion(value, exp_elem_ty, val_ty);

        // 3. Check the repeat count.
        let c_ty = self.check_expr(count, Some(TypeId::USIZE));
        let c_ty_id = self.resolve_tv(c_ty);
        if !self.ctx.type_registry.is_integer(c_ty_id) {
            self.ctx
                .struct_error(count.span, "repeat count must be an integer")
                .emit();
        }

        // 4. Return the final array type.
        if preserve_slice_ty {
            expected
        } else if is_infer {
            let mut ce = ConstEvaluator::new(self.ctx);
            let Ok(actual_len) = ce.eval_usize(count) else {
                return TypeId::ERROR;
            };
            if actual_len > u32::MAX as u64 {
                self.ctx
                    .struct_error(
                        count.span,
                        format!(
                            "array length {} exceeds the current compiler limit of {} elements",
                            actual_len,
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
                elem: exp_elem_ty,
                len: ConstGeneric::Value(ConstGenericValue {
                    ty: TypeId::USIZE,
                    kind: ConstGenericValueKind::Int(actual_len as i128),
                }),
            })
        } else {
            expected
        }
    }

    /// Helper 3: validate struct and union initialization.
    fn check_struct_or_union_literal(
        &mut self,
        init_fields: &[ast::StructFieldInit],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. Extract definition metadata and bound generics, then identify struct vs. union.
        let (def_fields, def_name, def_generics, generic_args, is_union): StructLiteralDefInfo =
            if let TypeKind::Def(def_id, args) = self.ctx.type_registry.get(exp_norm) {
                match &self.ctx.defs[def_id.0 as usize] {
                    Def::Struct(s) => {
                        let fields = s
                            .fields
                            .iter()
                            .map(|field| StructLiteralFieldInfo {
                                name: field.name,
                                ty: self.ctx.node_type_or_error(field.type_node.id),
                                has_default: field.default_value.is_some(),
                                definition_span: Some(field.name_span),
                                vis: Some(field.vis),
                                owner_def: Some(*def_id),
                            })
                            .collect();
                        (
                            fields,
                            self.ctx.resolve(s.name).to_string(),
                            s.generics.clone(),
                            args.clone(),
                            false,
                        )
                    }
                    Def::Union(u) => {
                        let fields = u
                            .fields
                            .iter()
                            .map(|field| StructLiteralFieldInfo {
                                name: field.name,
                                ty: self.ctx.node_type_or_error(field.type_node.id),
                                has_default: false,
                                definition_span: Some(field.name_span),
                                vis: Some(field.vis),
                                owner_def: Some(*def_id),
                            })
                            .collect();
                        (
                            fields,
                            self.ctx.resolve(u.name).to_string(),
                            u.generics.clone(),
                            args.clone(),
                            true,
                        )
                    }
                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "expected a struct or union type for literal initialization",
                            )
                            .emit();
                        return TypeId::ERROR;
                    }
                }
            } else if let TypeKind::AnonymousStruct(_, fields) =
                self.ctx.type_registry.get(exp_norm)
            {
                let defs: Vec<_> = fields
                    .iter()
                    .map(|field| StructLiteralFieldInfo {
                        name: field.name,
                        ty: field.ty,
                        has_default: false,
                        definition_span: None,
                        vis: None,
                        owner_def: None,
                    })
                    .collect();
                (
                    defs,
                    self.ctx.ty_to_string(exp_norm),
                    Vec::new(),
                    Vec::new(),
                    false,
                )
            } else if let TypeKind::AnonymousUnion(_, fields) = self.ctx.type_registry.get(exp_norm)
            {
                let defs: Vec<_> = fields
                    .iter()
                    .map(|field| StructLiteralFieldInfo {
                        name: field.name,
                        ty: field.ty,
                        has_default: false,
                        definition_span: None,
                        vis: None,
                        owner_def: None,
                    })
                    .collect();
                (
                    defs,
                    self.ctx.ty_to_string(exp_norm),
                    Vec::new(),
                    Vec::new(),
                    true,
                )
            } else {
                self.ctx
                    .struct_error(
                        span,
                        "expected a struct or union type for literal initialization",
                    )
                    .emit();
                return TypeId::ERROR;
            };

        let mut initialized = std::collections::HashSet::new();

        // 2. Check the types of user-provided field initializers.
        for init_f in init_fields {
            if self.is_canceled() {
                return TypeId::ERROR;
            }
            if let Some(def_f) = def_fields.iter().find(|f| f.name == init_f.name) {
                if let Some(definition_span) = def_f.definition_span {
                    self.ctx
                        .record_identifier_reference(init_f.name_span, definition_span);
                }
                if let (Some(vis), Some(owner_def)) = (def_f.vis, def_f.owner_def) {
                    let current_module = self.cached_current_module_id();
                    if !self
                        .ctx
                        .field_visibility_allows_access(vis, owner_def, current_module)
                    {
                        initialized.insert(init_f.name);
                        let name_str = self.ctx.resolve(init_f.name);
                        self.ctx
                            .struct_error(
                                init_f.span,
                                format!("field `{}` of type `{}` is private", name_str, def_name),
                            )
                            .with_hint(
                                "widen the field visibility, or construct the value from a module allowed by its visibility",
                            )
                            .emit();
                        continue;
                    }
                }
                let mut f_ty = def_f.ty;

                if f_ty == TypeId::ERROR {
                    self.ctx.struct_error(init_f.span, "internal compiler error: field type was unresolved prior to Typeck")
                        .with_hint("this is usually caused by a failing type resolver that missed emitting a diagnostic")
                        .emit();
                }

                if !def_generics.is_empty() && !generic_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in def_generics.iter().enumerate() {
                        map.insert(param.name, generic_args[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    f_ty = subst.substitute(f_ty);
                }

                let val_ty = self.check_expr(&init_f.value, Some(f_ty));
                self.check_coercion(&init_f.value, f_ty, val_ty);

                // Reject duplicate field initialization for both structs and unions.
                if !initialized.insert(init_f.name) {
                    let name_str = self.ctx.resolve(init_f.name);
                    self.ctx
                        .struct_error(
                            init_f.span,
                            format!("field `{}` is initialized more than once", name_str),
                        )
                        .emit();
                }
            } else {
                let name_str = self.ctx.resolve(init_f.name);
                self.ctx
                    .struct_error(
                        init_f.span,
                        format!("field `{}` does not exist in `{}`", name_str, def_name),
                    )
                    .emit();
            }
        }

        // 3. Enforce Kern-specific construction rules for structs and unions.
        if is_union {
            // Unions must initialize exactly one field.
            if initialized.len() != 1 {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "union `{}` must be initialized with exactly one field",
                            def_name
                        ),
                    )
                    .with_hint(format!("you provided {} fields", initialized.len()))
                    .with_hint(
                        "unions share memory across fields, so multiple initializers are ambiguous",
                    )
                    .emit();
            }
        } else {
            // Structs do not get implicit zero initialization.
            for def_f in &def_fields {
                if self.is_canceled() {
                    return TypeId::ERROR;
                }
                if !initialized.contains(&def_f.name) && !def_f.has_default {
                    let name_str = self.ctx.resolve(def_f.name).to_string();
                    self.ctx.struct_error(span, format!("field `{}` is missing and has no default value", name_str))
                        .with_hint("Kern structs do not zero-initialize implicitly.")
                        .with_hint(format!("write `{name_str}: undef` with an expected field type if you intentionally want to leave memory uninitialized"))
                        .emit();
                }
            }
        }

        expected
    }

    /// Helper 4: validate scalar construction forms like `.{ 10 }`.
    fn check_scalar_literal(&mut self, inner: &Expr, expected: TypeId) -> TypeId {
        let expected_norm = self.resolve_tv(expected);
        if (self.ctx.type_registry.is_integer(expected_norm)
            || self.ctx.type_registry.is_float(expected_norm))
            && !matches!(inner.kind, ExprKind::Undef)
        {
            let expected_str = self.ctx.ty_to_string(expected_norm);
            self.ctx
                .struct_error(
                    inner.span,
                    format!(
                        "numeric scalar value does not match `{}` syntax",
                        expected_str
                    ),
                )
                .with_hint("write a numeric literal suffix such as `10i32`, `10usize`, or `1.0f32`")
                .with_hint("or provide a type annotation and use a plain numeric literal")
                .emit();
            return TypeId::ERROR;
        }

        let inner_ty = self.check_expr(inner, Some(expected));
        self.check_coercion(inner, expected, inner_ty);
        expected
    }

    pub(crate) fn check_undef(&mut self, expected_ty: Option<TypeId>, span: Span) -> TypeId {
        match expected_ty {
            Some(ty) => ty,
            None => {
                self.ctx
                    .struct_error(span, "`undef` must have a known expected type context")
                    .emit();
                TypeId::ERROR
            }
        }
    }
}
