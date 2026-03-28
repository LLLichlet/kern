use super::ExprChecker;
use crate::checker::{ConstEvaluator, Substituter};
use crate::def::Def;
use crate::passes::TypeResolver;
use crate::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

type StructLiteralDefInfo = (
    Vec<(kernc_utils::SymbolId, TypeId, bool)>,
    String,
    Vec<ast::GenericParam>,
    Vec<TypeId>,
    bool,
);

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn resolve_data_init_target_type(
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
            if let Some(payload_ty) = v.payload_ty {
                let val_ty = self.check_expr(&init_f.value, Some(payload_ty));
                self.check_coercion(&init_f.value, payload_ty, val_ty);
            } else {
                let v_str = self.ctx.resolve(v.name).to_string();
                self.ctx
                    .struct_error(
                        init_f.span,
                        format!("variant `{}` does not take a payload", v_str),
                    )
                    .with_hint(format!("initialize it as `.{{ {} }}` instead", v_str))
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

    pub(crate) fn check_integer(&mut self, _expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        // 默认 fallback 为 usize
        let mut res_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Primitive(PrimitiveType::USize));

        if let Some(exp) = expected_ty {
            let norm = self.resolve_tv(exp);
            let kind = self.ctx.type_registry.get(norm).clone();

            // 如果上下文期望一个整数或浮点数，直接复用期望的类型
            if self.ctx.type_registry.is_integer(norm)
                || self.ctx.type_registry.is_float(norm)
                || matches!(
                    kind,
                    TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
                )
            {
                res_ty = exp;
            }
        }
        res_ty
    }

    pub(crate) fn check_float(&mut self, _expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        // 默认 fallback 为 f64
        let mut res_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Primitive(PrimitiveType::F64));

        if let Some(exp) = expected_ty {
            let norm = self.resolve_tv(exp);
            // 如果上下文期望一个浮点数，直接复用
            if self.ctx.type_registry.is_float(norm) {
                res_ty = exp;
            }
        }
        res_ty
    }

    pub(crate) fn check_data_init_expr(
        &mut self,
        type_node: Option<&ast::TypeNode>,
        literal: &ast::DataLiteralKind,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let target_ty = self.resolve_data_init_target_type(type_node, expected_ty, span);
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
        let exp_norm = self.resolve_tv(expected);
        let kind_enum = self.ctx.type_registry.get(exp_norm).clone();

        // 拦截胖指针构造（Trait Object 和 Closure Object）
        if let TypeKind::Pointer { is_mut, elem } | TypeKind::VolatilePtr { is_mut, elem } =
            kind_enum
        {
            let elem_norm = self.resolve_tv(elem);
            let target_inner_kind = self.ctx.type_registry.get(elem_norm).clone();
            // 提取唯一参数
            let inner_expr_opt: Option<&Expr> = match kind {
                ast::DataLiteralKind::Scalar(inner) => Some(inner.as_ref()),
                ast::DataLiteralKind::Struct(fields) if fields.len() == 1 => Some(&fields[0].value),
                _ => None,
            };

            match target_inner_kind {
                TypeKind::TraitObject(..) => {
                    if let Some(inner) = inner_expr_opt {
                        return self
                            .check_trait_object_init(inner, expected, elem_norm, is_mut, span);
                    } else {
                        self.ctx
                            .struct_error(
                                span,
                                "trait objects must be initialized with a single pointer",
                            )
                            .with_hint("example: `*mut Reader.{ file_ptr }`")
                            .emit();
                        return TypeId::ERROR;
                    }
                }
                TypeKind::ClosureInterface { .. } => {
                    if let Some(inner) = inner_expr_opt {
                        return self
                            .check_closure_object_init(inner, expected, elem_norm, is_mut, span);
                    } else {
                        self.ctx.struct_error(span, "invalid closure fat pointer construction")
                            .with_hint("expected syntax: `*mut Fn(...).{ raw_pointer }` or `*Fn(...).{ raw_pointer }`")
                            .with_hint("the raw pointer must explicitly be a pointer to the closure's anonymous state")
                            .emit();
                        return TypeId::ERROR;
                    }
                }
                _ => {} // 如果只是普通指针 (比如 *i32)，则放行往下走普通的结构体/数组检查
            }
        }

        // 拦截 void.{} 构造
        if self.ctx.type_registry.is_void(exp_norm) {
            match kind {
                // 空数组 fallback 或空结构体 fallback 都是合法的
                ast::DataLiteralKind::Array(elems) if elems.is_empty() => return expected,
                ast::DataLiteralKind::Struct(fields) if fields.is_empty() => return expected,
                _ => {
                    self.ctx.struct_error(span, "`void` is a zero-sized type and can only be initialized with empty braces `.{}`").emit();
                    return TypeId::ERROR;
                }
            }
        }

        // 统一识别 Enum 类型
        let is_data = matches!(kind_enum, TypeKind::Enum(..) | TypeKind::AnonymousEnum(..));

        match kind {
            ast::DataLiteralKind::Array(elems) => {
                let is_target_array_like = matches!(
                    kind_enum,
                    TypeKind::Array { .. } | TypeKind::ArrayInfer { .. } | TypeKind::Slice { .. }
                );
                if elems.is_empty() && !is_target_array_like {
                    if is_data {
                        self.check_enum_payload_literal(&[], expected, exp_norm, span)
                    } else {
                        self.check_struct_or_union_literal(&[], expected, exp_norm, span)
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
                } else {
                    self.check_struct_or_union_literal(init_fields, expected, exp_norm, span)
                }
            }
            ast::DataLiteralKind::Scalar(inner) => {
                if is_data {
                    if let ExprKind::Identifier(variant_name) = &inner.kind {
                        self.check_enum_literal(*variant_name, Some(expected), inner.span)
                    } else {
                        self.ctx
                            .struct_error(
                                inner.span,
                                "expected a simple variant name for data literal",
                            )
                            .emit();
                        TypeId::ERROR
                    }
                } else {
                    self.check_scalar_literal(inner, expected)
                }
            }
        }
    }

    /// 统一处理 `.Variant` 简写和 `.{ Variant }` 无负载初始化的校验
    pub(crate) fn check_enum_literal(
        &mut self,
        variant_name: SymbolId,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let mut res_ty = TypeId::ERROR;
        if let Some(exp_ty) = expected_ty {
            let norm_exp = self.resolve_tv(exp_ty);
            if let TypeKind::Enum(def_id, _) = self.ctx.type_registry.get(norm_exp) {
                if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
                    if let Some(v) = d.variants.iter().find(|v| v.name == variant_name) {
                        // 如果有 payload，必须使用 Struct() 初始化，不能用这种标量形式
                        if v.payload_type.is_some() {
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
                    if v.payload_ty.is_some() {
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

    /// 专门处理带有负载的 Enum 初始化，例如 `Result.{ Ok: 10 }`
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
            if let Some(payload_ast) = &v.payload_type {
                let mut payload_ty = self
                    .ctx
                    .node_types
                    .get(&payload_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

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
                self.ctx
                    .struct_error(
                        init_f.span,
                        format!("variant `{}` does not take a payload", v_str),
                    )
                    .with_hint(format!("initialize it as `.{{ {} }}` instead", v_str))
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

    /// 辅助方法 1：校验普通数组字面量 `.{ 1, 2, 3 }`
    fn check_array_literal(
        &mut self,
        elems: &[Expr],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. 动态剥离类型信息
        let (exp_elem_ty, expected_len, exp_is_mut) = match self.ctx.type_registry.get(exp_norm) {
            TypeKind::Array { elem, len, is_mut } => (*elem, Some(*len), *is_mut),
            TypeKind::ArrayInfer { elem, is_mut } => (*elem, None, *is_mut),
            TypeKind::Slice { elem, is_mut } => (*elem, None, *is_mut),
            _ => {
                let ty_str = self.ctx.ty_to_string(expected);
                self.ctx
                    .struct_error(
                        span,
                        "expected an array or slice type for literal `.{ ... }`",
                    )
                    .with_hint(format!("context expects `{}`", ty_str))
                    .emit();
                return TypeId::ERROR;
            }
        };

        // 2. 如果是定长数组，校验长度
        if let Some(len) = expected_len
            && elems.len() as u64 != len
        {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "array literal length ({}) does not match expected length ({})",
                        elems.len(),
                        len
                    ),
                )
                .emit();
        }

        // 3. 校验所有元素的类型
        for e in elems {
            let act_ty = self.check_expr(e, Some(exp_elem_ty));
            self.check_coercion(e, exp_elem_ty, act_ty);
        }

        // 4. 返回最终确定的类型
        if expected_len.is_none() {
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
                is_mut: exp_is_mut,
                elem: exp_elem_ty,
                len: actual_len,
            })
        } else {
            // 原本就是 [N]T
            expected
        }
    }

    /// 辅助方法 2：校验重复数组字面量 `.{ 0; 1024 }`
    fn check_repeat_literal(
        &mut self,
        value: &Expr,
        count: &Expr,
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. 动态剥离类型信息
        let (exp_elem_ty, is_infer, exp_is_mut) = match self.ctx.type_registry.get(exp_norm) {
            TypeKind::Array { elem, is_mut, .. } => (*elem, false, *is_mut),
            TypeKind::ArrayInfer { elem, is_mut } => (*elem, true, *is_mut),
            TypeKind::Slice { elem, is_mut } => (*elem, true, *is_mut),
            _ => {
                let ty_str = self.ctx.ty_to_string(expected);
                self.ctx
                    .struct_error(
                        span,
                        "expected an array or slice type for repeat literal `.{ v; N }`",
                    )
                    .with_hint(format!("context expects `{}`", ty_str))
                    .emit();
                return TypeId::ERROR;
            }
        };

        // 2. 校验重复的元素值
        let val_ty = self.check_expr(value, Some(exp_elem_ty));
        self.check_coercion(value, exp_elem_ty, val_ty);

        // 3. 校验重复次数
        let c_ty = self.check_expr(count, Some(TypeId::USIZE));
        let c_ty_id = self.resolve_tv(c_ty);
        if !self.ctx.type_registry.is_integer(c_ty_id) {
            self.ctx
                .struct_error(count.span, "repeat count must be an integer")
                .emit();
        }

        // 4. 返回最终类型
        if is_infer {
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
                is_mut: exp_is_mut,
                elem: exp_elem_ty,
                len: actual_len,
            })
        } else {
            expected
        }
    }

    /// 辅助方法 3：校验结构体或联合体初始化 `.{ x: 10, y: 20 }` 或 Union `.{ as_int: 123 }`
    fn check_struct_or_union_literal(
        &mut self,
        init_fields: &[ast::StructFieldInit],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. 提取定义信息与泛型实参，同时识别是 Struct 还是 Union
        let (def_fields, def_name, def_generics, generic_args, is_union): StructLiteralDefInfo =
            if let TypeKind::Def(def_id, args) = self.ctx.type_registry.get(exp_norm) {
                match &self.ctx.defs[def_id.0 as usize] {
                    Def::Struct(s) => {
                        let fields = s
                            .fields
                            .iter()
                            .map(|field| {
                                (
                                    field.name,
                                    self.ctx
                                        .node_types
                                        .get(&field.type_node.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR),
                                    field.default_value.is_some(),
                                )
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
                            .map(|field| {
                                (
                                    field.name,
                                    self.ctx
                                        .node_types
                                        .get(&field.type_node.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR),
                                    false,
                                )
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
                    .map(|field| (field.name, field.ty, false))
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
                    .map(|field| (field.name, field.ty, false))
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

        // 2. 校验用户提供的初始化字段的类型
        for init_f in init_fields {
            if let Some(def_f) = def_fields.iter().find(|f| f.0 == init_f.name) {
                let mut f_ty = def_f.1;

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

                // 检查重复初始化的字段（对 struct 和 union 都有效）
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

        // 3. 校验 Kern 核心规则：针对 Struct 和 Union 分别处理
        if is_union {
            // Kern Union 规则：必须且只能初始化 1 个字段
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
            // Kern Struct 规则：无隐式零初始化。漏掉字段必须显式使用 undef 或具有默认值
            for def_f in &def_fields {
                if !initialized.contains(&def_f.0) && !def_f.2 {
                    let name_str = self.ctx.resolve(def_f.0).to_string();
                    self.ctx.struct_error(span, format!("field `{}` is missing and has no default value", name_str))
                        .with_hint("Kern structs do not zero-initialize implicitly.")
                        .with_hint(format!("use `{}: type.{{undef}}` if you intentionally want to leave memory uninitialized", name_str))
                        .emit();
                }
            }
        }

        expected
    }

    /// 辅助方法 4：校验标量构造 `.{ 10 }`
    fn check_scalar_literal(&mut self, inner: &Expr, expected: TypeId) -> TypeId {
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

    fn check_trait_object_init(
        &mut self,
        inner: &Expr,
        expected_ptr_ty: TypeId,
        trait_norm: TypeId,
        is_mut_expected: bool,
        span: Span,
    ) -> TypeId {
        let inner_ty = self.check_expr(inner, None);
        if inner_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let inner_ty_id = self.resolve_tv(inner_ty);

        // 1. 必须传入指针
        let is_inner_ptr_mut = match self.ctx.type_registry.get(inner_ty_id) {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => *is_mut,
            _ => {
                self.ctx
                    .struct_error(
                        inner.span,
                        "trait objects can only be constructed from pointers",
                    )
                    .emit();
                return TypeId::ERROR;
            }
        };

        // 2. 不允许把不可变指针塞进可变胖指针
        if is_mut_expected && !is_inner_ptr_mut {
            self.ctx
                .struct_error(
                    inner.span,
                    "cannot create a mutable trait object from an immutable pointer",
                )
                .with_hint(
                    "expected a mutable pointer (like `val..&`), but found an immutable pointer",
                )
                .emit();
            return TypeId::ERROR;
        }

        // 3. 校验方法契约
        if !self.check_trait_impl(inner_ty_id, trait_norm) {
            self.ctx
                .struct_error(
                    span,
                    "the provided pointer type does not implement the target trait",
                )
                .emit();
            return TypeId::ERROR;
        }

        // 4. 返回构造好的胖指针类型
        expected_ptr_ty
    }

    pub(crate) fn check_closure_object_init(
        &mut self,
        inner: &Expr,
        expected_ptr_ty: TypeId,
        closure_interface_norm: TypeId,
        is_mut_expected: bool,
        span: Span,
    ) -> TypeId {
        let inner_ty = self.check_expr(inner, None);
        if inner_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let inner_ty_id = self.resolve_tv(inner_ty);

        let is_inner_ptr_mut = match self.ctx.type_registry.get(inner_ty_id) {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => *is_mut,
            _ => {
                self.ctx
                    .struct_error(
                        inner.span,
                        "closure objects can only be constructed from pointers",
                    )
                    .emit();
                return TypeId::ERROR;
            }
        };

        if is_mut_expected && !is_inner_ptr_mut {
            self.ctx
                .struct_error(
                    inner.span,
                    "cannot create a mutable closure object from an immutable pointer",
                )
                .emit();
            return TypeId::ERROR;
        }

        let interface_kind = self.ctx.type_registry.get(closure_interface_norm).clone();
        let (interface_params, interface_ret) = match interface_kind {
            TypeKind::ClosureInterface { params, ret } => (params, ret),
            _ => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: closure object init expected `ClosureInterface`, found `{}`.",
                        self.ctx.ty_to_string(closure_interface_norm)
                    ),
                );
                return TypeId::ERROR;
            }
        };

        let inner_elem_ty = self
            .ctx
            .type_registry
            .get_elem_type(inner_ty_id)
            .unwrap_or(TypeId::ERROR);
        let inner_elem_ty_id = self.resolve_tv(inner_elem_ty);
        let inner_elem_norm = self.ctx.type_registry.normalize(inner_elem_ty_id);
        let inner_kind = self.ctx.type_registry.get(inner_elem_norm).clone();

        match inner_kind {
            TypeKind::AnonymousState {
                params: state_params,
                ret: state_ret,
                ..
            } => {
                if interface_params.len() != state_params.len() {
                    self.ctx
                        .struct_error(
                            span,
                            "closure signature mismatch: incorrect number of parameters",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                for (exp_p, act_p) in interface_params.iter().zip(state_params.iter()) {
                    if self.resolve_tv(*exp_p) != self.resolve_tv(*act_p) {
                        self.ctx
                            .struct_error(span, "closure parameter mismatch")
                            .emit();
                        return TypeId::ERROR;
                    }
                }
                if self.resolve_tv(interface_ret) != self.resolve_tv(state_ret) {
                    self.ctx
                        .struct_error(span, "closure return type mismatch")
                        .emit();
                    return TypeId::ERROR;
                }
                expected_ptr_ty
            }
            _ => {
                let actual_ty_str = self.ctx.ty_to_string(inner_elem_norm);
                self.ctx
                    .struct_error(inner.span, "expected a closure anonymous state pointer")
                    .with_hint(format!("found pointer to `{}`", actual_ty_str))
                    .emit();
                TypeId::ERROR
            }
        }
    }
}
