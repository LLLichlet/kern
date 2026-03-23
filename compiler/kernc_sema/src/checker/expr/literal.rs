use super::ExprChecker;
use crate::checker::{ConstEvaluator, Substituter};
use crate::def::Def;
use crate::passes::TypeResolver;
use crate::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
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
            if self.ctx.type_registry.is_integer(norm) || self.ctx.type_registry.is_float(norm) {
                res_ty = exp;
            }
            // 如果上下文明确期望一个指针，让这个整数直接吸收该指针类型
            else if matches!(
                kind,
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            ) {
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
        // 智能决定目标类型
        let target_ty = if let Some(ty_ast) = type_node {
            // 情况 A: 显式指定了类型前缀 (如 Result[i32, i32].{ ... })
            let mut resolver = TypeResolver::new(self.ctx);
            let scope = resolver.ctx.scopes.current_scope_id().unwrap();
            resolver.resolve_type(ty_ast, scope)
        } else if let Some(exp) = expected_ty {
            // 情况 B: 省略了前缀，但外层有期望的类型 (如 `(ret_type)` Option[i32] = .{ ... })
            // 去除 Mut 修饰符，拿到真正的数据类型
            self.resolve_tv(exp)
        } else {
            // 情况 C: 既没写前缀，外层又不知道该是什么类型 (如 let x = .{ 10 })
            self.ctx.struct_error(span, "cannot infer type for anonymous initialization `.{...}`")
                .with_hint("provide an explicit type context or prepend the type name, e.g., `MyStruct.{...}`")
                .emit();
            return TypeId::ERROR;
        };

        // 将确定的 target_ty 继续下传给具体的字面量检查器
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

        // 拦截 Trait Object 构造（*Trait.{ ... } 或 *mut Trait.{ ... }）
        if let TypeKind::Pointer { is_mut, elem } | TypeKind::VolatilePtr { is_mut, elem } =
            kind_enum
        {
            let elem_norm = self.resolve_tv(elem);
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(elem_norm) {
                if let ast::DataLiteralKind::Scalar(inner) = kind {
                    // 进入胖指针构造器
                    return self.check_trait_object_init(inner, expected, elem_norm, is_mut, span);
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
        }

        // 🌟 统一识别 Enum 类型
        let is_data = matches!(kind_enum, TypeKind::Enum(..));

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
                    // 如果是 `.{ None }` 形式，直接提取出变体名，复用简写逻辑！
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
        let (def_id, generic_args) =
            if let TypeKind::Enum(id, args) = self.ctx.type_registry.get(exp_norm) {
                (*id, args.clone())
            } else {
                unreachable!()
            };

        let data_def = match &self.ctx.defs[def_id.0 as usize] {
            Def::Enum(d) => d.clone(),
            _ => unreachable!(),
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
                self.check_coercion(init_f.span, payload_ty, val_ty);
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
        if let Some(len) = expected_len {
            if elems.len() as u64 != len {
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
        }

        // 3. 校验所有元素的类型
        for e in elems {
            let act_ty = self.check_expr(e, Some(exp_elem_ty));
            self.check_coercion(e.span, exp_elem_ty, act_ty);
        }

        // 4. 返回最终确定的类型
        if expected_len.is_none() {
            self.ctx.type_registry.intern(TypeKind::Array {
                is_mut: exp_is_mut,
                elem: exp_elem_ty,
                len: elems.len() as u64,
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
        self.check_coercion(value.span, exp_elem_ty, val_ty);

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
            let actual_len = match ce.eval_usize(count) {
                Ok(val) => val,
                Err(_) => 0, // 兜底填0
            };

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
        let (def_fields, def_name, def_generics, generic_args, is_union) =
            if let TypeKind::Def(def_id, args) = self.ctx.type_registry.get(exp_norm) {
                match &self.ctx.defs[def_id.0 as usize] {
                    Def::Struct(s) => (
                        s.fields.clone(),
                        self.ctx.resolve(s.name).to_string(),
                        s.generics.clone(),
                        args.clone(),
                        false,
                    ),
                    Def::Union(u) => (
                        u.fields.clone(),
                        self.ctx.resolve(u.name).to_string(),
                        u.generics.clone(),
                        args.clone(),
                        true,
                    ),
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
            if let Some(def_f) = def_fields.iter().find(|f| f.name == init_f.name) {
                let mut f_ty = self
                    .ctx
                    .node_types
                    .get(&def_f.type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                // 如果结构体本身的字段类型就是错的，必须强制抛出异常阻断编译
                if f_ty == TypeId::ERROR {
                    self.ctx.struct_error(init_f.span, "internal compiler error: field type was unresolved prior to Typeck")
                        .with_hint("this is usually caused by a failing type resolver that missed emitting a diagnostic")
                        .emit();
                }

                // 处理泛型字段类型替换
                if !def_generics.is_empty() && !generic_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in def_generics.iter().enumerate() {
                        map.insert(param.name, generic_args[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    f_ty = subst.substitute(f_ty);
                }

                let val_ty = self.check_expr(&init_f.value, Some(f_ty));
                self.check_coercion(init_f.span, f_ty, val_ty);

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
                if !initialized.contains(&def_f.name) && def_f.default_value.is_none() {
                    let name_str = self.ctx.resolve(def_f.name).to_string();
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
        self.check_coercion(inner.span, expected, inner_ty);
        expected
    }

    pub(crate) fn check_undef(&mut self, expected_ty: Option<TypeId>, span: Span) -> TypeId {
        if expected_ty.is_none() {
            self.ctx
                .struct_error(span, "`undef` must have a known expected type context")
                .emit();
            TypeId::ERROR
        } else {
            expected_ty.unwrap()
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
}
