use super::*;

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    pub(super) fn eval_data_init(
        &mut self,
        expr: &Expr,
        literal: &ast::DataLiteralKind,
        depth: usize,
    ) -> ConstEvalResult<ConstValue> {
        let norm_target = self.expr_type(expr);

        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Enum(def_id, _) => {
                self.eval_named_enum_data_init(def_id, literal, depth, expr.span)
            }
            TypeKind::AnonymousEnum(enum_def) => {
                self.eval_anon_enum_data_init(&enum_def, literal, depth, expr.span)
            }
            _ => match literal {
                ast::DataLiteralKind::Scalar(inner) => self.eval_inner(inner, depth + 1),
                ast::DataLiteralKind::Array(elems) => {
                    let mut arr = Vec::new();
                    for e in elems {
                        arr.push(self.eval_inner(e, depth + 1)?);
                    }
                    Ok(ConstValue::Array(arr))
                }
                ast::DataLiteralKind::Struct(fields) => {
                    let mut map = HashMap::new();
                    for f in fields {
                        map.insert(f.name, self.eval_inner(&f.value, depth + 1)?);
                    }
                    Ok(ConstValue::Struct(map))
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    let val = self.eval_inner(value, depth + 1)?;
                    let cnt = self.eval_usize(count)?;
                    Ok(ConstValue::Array(vec![val; cnt as usize]))
                }
            },
        }
    }

    pub(super) fn eval_named_enum_data_init(
        &mut self,
        def_id: crate::def::DefId,
        literal: &ast::DataLiteralKind,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some(Def::Enum(enum_def)) = self.ctx.defs.get(def_id.0 as usize).cloned() else {
            return Err(ConstEvalError);
        };

        match literal {
            ast::DataLiteralKind::Scalar(inner) => {
                let Some(variant_name) = self.enum_ctor_variant_name(inner, span) else {
                    return Err(ConstEvalError);
                };
                let Some((variant, tag)) =
                    self.named_enum_variant_and_tag(&enum_def, variant_name, depth, span)
                else {
                    return Err(ConstEvalError);
                };
                if variant.payload_type.is_some() {
                    self.ctx
                        .struct_error(
                            inner.span,
                            format!(
                                "variant `{}` requires a payload in constant initialization",
                                self.ctx.resolve(variant_name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                }

                if enum_def.variants.iter().all(|v| v.payload_type.is_none()) {
                    Ok(ConstValue::Int(tag))
                } else {
                    Ok(ConstValue::Enum { tag, payload: None })
                }
            }
            ast::DataLiteralKind::Struct(fields) => {
                if fields.len() != 1 {
                    self.ctx
                        .struct_error(
                            span,
                            "enum constant initialization must specify exactly one variant",
                        )
                        .emit();
                    return Err(ConstEvalError);
                }
                let init = &fields[0];
                let Some((variant, tag)) =
                    self.named_enum_variant_and_tag(&enum_def, init.name, depth, init.span)
                else {
                    return Err(ConstEvalError);
                };
                let Some(_) = variant.payload_type else {
                    self.ctx
                        .struct_error(
                            init.span,
                            format!(
                                "variant `{}` does not take a payload in constant initialization",
                                self.ctx.resolve(init.name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                };
                let payload = self.eval_inner(&init.value, depth + 1)?;
                Ok(ConstValue::Enum {
                    tag,
                    payload: Some(Box::new(payload)),
                })
            }
            _ => {
                self.ctx
                    .struct_error(span, "invalid enum constant initializer")
                    .with_hint("use `Type.{ Variant }` or `Type.{ Variant: payload }`")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn eval_anon_enum_data_init(
        &mut self,
        enum_def: &crate::ty::AnonymousEnum,
        literal: &ast::DataLiteralKind,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        match literal {
            ast::DataLiteralKind::Scalar(inner) => {
                let Some(variant_name) = self.enum_ctor_variant_name(inner, span) else {
                    return Err(ConstEvalError);
                };
                let Some((variant, tag)) =
                    self.anon_enum_variant_and_tag(enum_def, variant_name, span)
                else {
                    return Err(ConstEvalError);
                };
                if variant.payload_ty.is_some() {
                    self.ctx
                        .struct_error(
                            inner.span,
                            format!(
                                "variant `{}` requires a payload in constant initialization",
                                self.ctx.resolve(variant_name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                }

                if enum_def.variants.iter().all(|v| v.payload_ty.is_none()) {
                    Ok(ConstValue::Int(tag))
                } else {
                    Ok(ConstValue::Enum { tag, payload: None })
                }
            }
            ast::DataLiteralKind::Struct(fields) => {
                if fields.len() != 1 {
                    self.ctx
                        .struct_error(
                            span,
                            "enum constant initialization must specify exactly one variant",
                        )
                        .emit();
                    return Err(ConstEvalError);
                }
                let init = &fields[0];
                let Some((variant, tag)) =
                    self.anon_enum_variant_and_tag(enum_def, init.name, init.span)
                else {
                    return Err(ConstEvalError);
                };
                let Some(_) = variant.payload_ty else {
                    self.ctx
                        .struct_error(
                            init.span,
                            format!(
                                "variant `{}` does not take a payload in constant initialization",
                                self.ctx.resolve(init.name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                };
                let payload = self.eval_inner(&init.value, depth + 1)?;
                Ok(ConstValue::Enum {
                    tag,
                    payload: Some(Box::new(payload)),
                })
            }
            _ => {
                self.ctx
                    .struct_error(span, "invalid enum constant initializer")
                    .with_hint("use `Type.{ Variant }` or `Type.{ Variant: payload }`")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn enum_ctor_variant_name(&mut self, inner: &Expr, span: Span) -> Option<SymbolId> {
        match inner.kind {
            ExprKind::Identifier(name) | ExprKind::EnumLiteral(name) => Some(name),
            _ => {
                self.ctx
                    .struct_error(span, "enum constant initialization expects a variant name")
                    .with_hint("write `Type.{ Variant }` for payload-less variants")
                    .emit();
                None
            }
        }
    }

    pub(super) fn named_enum_variant_and_tag(
        &mut self,
        enum_def: &crate::def::EnumDef,
        variant_name: SymbolId,
        depth: usize,
        span: Span,
    ) -> Option<(ast::EnumVariant, i128)> {
        let mut current_val: i128 = 0;
        for variant in &enum_def.variants {
            if let Some(value_expr) = &variant.value
                && let Ok(ConstValue::Int(val)) = self.eval_inner(value_expr, depth + 1)
            {
                current_val = val;
            }
            if variant.name == variant_name {
                return Some((variant.clone(), current_val));
            }
            current_val += 1;
        }

        self.ctx
            .struct_error(
                span,
                format!(
                    "variant `.{}` not found in enum constant initialization",
                    self.ctx.resolve(variant_name)
                ),
            )
            .emit();
        None
    }

    pub(super) fn anon_enum_variant_and_tag(
        &mut self,
        enum_def: &crate::ty::AnonymousEnum,
        variant_name: SymbolId,
        span: Span,
    ) -> Option<(crate::ty::AnonymousVariant, i128)> {
        let mut current_val: i128 = 0;
        for variant in &enum_def.variants {
            if let Some(explicit_value) = variant.explicit_value {
                current_val = explicit_value;
            }
            if variant.name == variant_name {
                return Some((variant.clone(), current_val));
            }
            current_val += 1;
        }

        self.ctx
            .struct_error(
                span,
                format!(
                    "variant `.{}` not found in enum constant initialization",
                    self.ctx.resolve(variant_name)
                ),
            )
            .emit();
        None
    }

    pub(super) fn match_pattern(
        &mut self,
        pattern: &ast::MatchPattern,
        target_value: &ConstValue,
        target_ty: TypeId,
        depth: usize,
    ) -> ConstEvalResult<Option<HashMap<SymbolId, ConstValue>>> {
        match &pattern.kind {
            ast::MatchPatternKind::Value(expr) => {
                let value = self.eval_inner(expr, depth + 1)?;
                if value == *target_value {
                    Ok(Some(HashMap::new()))
                } else {
                    Ok(None)
                }
            }
            ast::MatchPatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start = self.eval_inner(start, depth + 1)?;
                let end = self.eval_inner(end, depth + 1)?;
                let matches = match (target_value, start, end) {
                    (ConstValue::Int(target), ConstValue::Int(start), ConstValue::Int(end)) => {
                        if *inclusive {
                            start <= *target && *target <= end
                        } else {
                            start <= *target && *target < end
                        }
                    }
                    _ => false,
                };
                if matches {
                    Ok(Some(HashMap::new()))
                } else {
                    Ok(None)
                }
            }
            ast::MatchPatternKind::Variant(variant) => self.match_variant_pattern(
                variant.variant_name,
                variant.binding.as_ref(),
                target_value,
                target_ty,
                depth,
                pattern.span,
            ),
            ast::MatchPatternKind::CatchAll => Ok(Some(HashMap::new())),
        }
    }

    pub(super) fn match_variant_pattern(
        &mut self,
        variant_name: SymbolId,
        binding: Option<&ast::BindingPattern>,
        target_value: &ConstValue,
        target_ty: TypeId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<Option<HashMap<SymbolId, ConstValue>>> {
        let expected_tag = match self.variant_tag(target_ty, variant_name, depth, span)? {
            Some(tag) => tag,
            None => return Ok(None),
        };

        let mut bindings = HashMap::new();
        match target_value {
            ConstValue::Enum { tag, payload } if *tag == expected_tag => {
                if let Some(binding) = binding
                    && let Some(payload) = payload
                {
                    bindings.insert(binding.name, payload.as_ref().clone());
                }
                Ok(Some(bindings))
            }
            ConstValue::Int(tag) if *tag == expected_tag => Ok(Some(bindings)),
            _ => Ok(None),
        }
    }

    pub(super) fn variant_payload_ty(
        &mut self,
        target_ty: TypeId,
        variant_name: SymbolId,
        _depth: usize,
        span: Span,
    ) -> ConstEvalResult<Option<TypeId>> {
        let norm = self.ctx.type_registry.normalize(target_ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Enum(def_id, generic_args) => {
                let Some(Def::Enum(def)) = self.ctx.defs.get(def_id.0 as usize).cloned() else {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (ConstEval): expected enum definition for DefId {}.",
                            def_id.0
                        ),
                    );
                    return Err(ConstEvalError);
                };

                let Some(variant) = def.variants.iter().find(|v| v.name == variant_name) else {
                    return Ok(None);
                };
                let Some(payload_ast) = &variant.payload_type else {
                    return Ok(None);
                };

                let mut payload_ty = self
                    .ctx
                    .node_types
                    .get(&payload_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                if !def.generics.is_empty() && !generic_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in def.generics.iter().enumerate() {
                        map.insert(param.name, generic_args[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    payload_ty = subst.substitute(payload_ty);
                }
                Ok(Some(payload_ty))
            }
            TypeKind::AnonymousEnum(def) => Ok(def
                .variants
                .iter()
                .find(|v| v.name == variant_name)
                .and_then(|variant| variant.payload_ty)),
            _ => Ok(None),
        }
    }

    pub(super) fn variant_tag(
        &mut self,
        target_ty: TypeId,
        variant_name: SymbolId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<Option<i128>> {
        match self.ctx.type_registry.get(target_ty).clone() {
            TypeKind::Enum(def_id, _) => {
                let Some(Def::Enum(enum_def)) = self.ctx.defs.get(def_id.0 as usize).cloned()
                else {
                    return Err(ConstEvalError);
                };
                let mut current_val = 0i128;
                for variant in enum_def.variants {
                    if let Some(value_expr) = &variant.value
                        && let Ok(ConstValue::Int(value)) = self.eval_inner(value_expr, depth + 1)
                    {
                        current_val = value;
                    }
                    if variant.name == variant_name {
                        return Ok(Some(current_val));
                    }
                    current_val += 1;
                }
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "variant `.{}` not found in enum",
                            self.ctx.resolve(variant_name)
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let mut current_val = 0i128;
                for variant in enum_def.variants {
                    if let Some(value) = variant.explicit_value {
                        current_val = value;
                    }
                    if variant.name == variant_name {
                        return Ok(Some(current_val));
                    }
                    current_val += 1;
                }
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "variant `.{}` not found in anonymous enum",
                            self.ctx.resolve(variant_name)
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
            _ => Ok(None),
        }
    }

    pub(super) fn eval_enum_literal(
        &mut self,
        node_id: NodeId,
        variant_name: SymbolId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let norm_ty = self.node_type(node_id);

        let def_id = if let TypeKind::Enum(id, _) = self.ctx.type_registry.get(norm_ty) {
            *id
        } else {
            self.ctx
                .struct_error(
                    span,
                    "variant literal type could not be resolved to a data type during constant evaluation",
                )
                .emit();
            return Err(ConstEvalError);
        };

        let data_def = if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
            d.clone()
        } else {
            return Err(ConstEvalError);
        };

        // 绂佹瀵瑰甫鏈?Payload 鐨?ADT 鍙樹綋杩涜甯搁噺鏁存暟姹傚€?
        for v in &data_def.variants {
            if v.payload_type.is_some() {
                self.ctx
                    .struct_error(
                        span,
                        "cannot evaluate ADT variants with payloads as integer constants",
                    )
                    .with_hint("only C-style `data` types (without payloads) can be implicitly evaluated to integers")
                    .emit();
                return Err(ConstEvalError);
            }
        }

        let mut current_val: i128 = 0;
        for v in data_def.variants {
            if let Some(v_expr) = v.value
                && let Ok(ConstValue::Int(val)) = self.eval_inner(&v_expr, depth + 1)
            {
                current_val = val;
            }
            if v.name == variant_name {
                return Ok(ConstValue::Int(current_val));
            }
            current_val += 1;
        }

        let v_str = self.ctx.resolve(variant_name).to_string();
        self.ctx
            .struct_error(span, format!("variant `.{}` not found in data type", v_str))
            .emit();
        Err(ConstEvalError)
    }
}
