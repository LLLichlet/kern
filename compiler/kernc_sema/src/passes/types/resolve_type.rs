use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub fn resolve_type(&mut self, ty_node: &ast::TypeNode, env_scope: ScopeId) -> TypeId {
        if let Some(cached_ty) = self.ctx.node_type(ty_node.id)
            && cached_ty != TypeId::ERROR
        {
            return cached_ty;
        }

        let ty_id = match &ty_node.kind {
            ast::TypeKind::Error => TypeId::ERROR,
            ast::TypeKind::Path { anchor, segments } => {
                self.resolve_path_type(*anchor, segments, env_scope, ty_node.span)
            }
            ast::TypeKind::Void => TypeId::VOID,
            ast::TypeKind::Optional { inner } => {
                let inner_ty = self.resolve_type(inner, env_scope);
                self.make_builtin_optional_type(inner_ty, ty_node.span)
            }
            ast::TypeKind::Result { ok, err } => {
                let ok_ty = self.resolve_type(ok, env_scope);
                let err_ty = self.resolve_type(err, env_scope);
                self.make_builtin_result_type(ok_ty, err_ty)
            }
            ast::TypeKind::Struct { is_extern, fields } => {
                let mut anon_fields =
                    self.resolve_anonymous_fields(fields, env_scope, ty_node.span, "struct", true);

                if !*is_extern {
                    anon_fields.sort_by_key(|f| f.name);
                }

                self.check_duplicate_anon_fields(&anon_fields, ty_node.span, "anonymous struct");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousStruct(*is_extern, anon_fields))
            }
            ast::TypeKind::Union { is_extern, fields } => {
                let mut anon_fields =
                    self.resolve_anonymous_fields(fields, env_scope, ty_node.span, "union", false);
                anon_fields.sort_by_key(|f| f.name);
                self.check_duplicate_anon_fields(&anon_fields, ty_node.span, "anonymous union");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(*is_extern, anon_fields))
            }
            ast::TypeKind::Enum {
                backing_type,
                variants,
            } => {
                let backing_ty = backing_type.as_ref().map(|bt| {
                    let resolved_ty = self.resolve_type(bt, env_scope);
                    if !self.ctx.type_registry.is_integer(resolved_ty)
                        && resolved_ty != TypeId::ERROR
                    {
                        self.ctx
                            .emit_error(bt.span, "anonymous enum backing type must be an integer");
                    }
                    resolved_ty
                });

                let mut anon_variants = Vec::new();
                for variant in variants {
                    let payload_ty = variant.payload_type.as_ref().map(|payload_ty| {
                        let resolved_ty = self.resolve_type(payload_ty, env_scope);
                        self.ensure_sized(resolved_ty, payload_ty.span);
                        resolved_ty
                    });

                    let explicit_value = variant.value.as_ref().map(|value_expr| {
                        self.resolve_expr(value_expr, env_scope);
                        self.ctx.scopes.set_current_scope(env_scope);
                        let mut evaluator = ConstEvaluator::new(self.ctx);
                        evaluator.eval_math(value_expr).unwrap_or(0)
                    });

                    anon_variants.push(AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty,
                        explicit_value,
                    });
                }

                self.check_duplicate_anon_variants(&anon_variants, ty_node.span);

                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                        backing_ty,
                        builtin: None,
                        variants: anon_variants,
                    }))
            }
            ast::TypeKind::Pointer { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::VolatilePtr { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Slice { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Array { elem, len } => {
                let base = self.resolve_type(elem, env_scope);
                let resolved_len =
                    self.resolve_const_generic_expr(len, TypeId::USIZE, env_scope, "array length");
                if matches!(resolved_len, ConstGeneric::Error) {
                    return TypeId::ERROR;
                }
                if let ConstGeneric::Value(value) = resolved_len
                    && let Some(value) = value.as_int()
                    && value > u32::MAX as i128
                {
                    self.ctx
                        .struct_error(
                            len.span,
                            format!(
                                "array length {} exceeds the current compiler limit of {} elements",
                                value,
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
                    elem: base,
                    len: resolved_len,
                })
            }
            ast::TypeKind::ArrayInfer { elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx
                    .type_registry
                    .intern(TypeKind::ArrayInfer { elem: base })
            }
            ast::TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::Function {
                    params: param_tys,
                    ret: ret_ty,
                    is_variadic: *is_variadic,
                })
            }
            ast::TypeKind::SelfType => {
                self.ctx.scopes.set_current_scope(env_scope);
                let self_sym = self.ctx.intern("Self");
                if let Some(info) = self.ctx.scopes.resolve_type_symbol(self_sym) {
                    info.type_id
                } else {
                    self.ctx
                        .struct_error(
                            ty_node.span,
                            "the `Self` type is only valid inside `impl` blocks or `trait` definitions",
                        )
                        .with_hint("you are using it in a global or standard function context")
                        .emit();
                    TypeId::ERROR
                }
            }
            ast::TypeKind::Never => TypeId::NEVER,
            ast::TypeKind::Infer => {
                self.ctx
                    .struct_error(ty_node.span, "type inference `_` is not allowed as a standalone type")
                    .with_hint("in Kern, the `_` placeholder is exclusively used for array length inference, e.g., `[_]u8.{ 1, 2, 3 }`")
                    .emit();
                TypeId::ERROR
            }
            ast::TypeKind::ClosureInterface { params, ret } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: param_tys,
                    ret: ret_ty,
                })
            }
            ast::TypeKind::TypeOf(expr) => {
                self.resolve_expr(expr, env_scope);
                TypeId::ERROR
            }
            _ => {
                self.ctx
                    .emit_error(ty_node.span, "Invalid or unsupported type construction");
                TypeId::ERROR
            }
        };

        self.ctx.set_node_type(ty_node.id, ty_id);
        ty_id
    }

    fn resolve_anonymous_fields(
        &mut self,
        fields: &[ast::StructFieldDef],
        env_scope: ScopeId,
        _span: Span,
        kind_name: &str,
        _allow_default_values: bool,
    ) -> Vec<AnonymousField> {
        let mut anon_fields = Vec::with_capacity(fields.len());

        for f in fields {
            let f_ty = self.resolve_type(&f.type_node, env_scope);
            self.ensure_sized(f_ty, f.type_node.span);

            if !f.vis.is_private() {
                let msg = format!("anonymous {} fields cannot be declared pub", kind_name);
                self.ctx
                    .struct_error(f.span, msg)
                    .with_hint(
                        "field-level `pub` is only supported on named declarations like `struct Name { ... }`",
                    )
                    .emit();
            }

            if f.default_value.is_some() {
                let msg = format!("anonymous {}s cannot have default field values", kind_name);
                self.ctx
                    .struct_error(f.span, msg)
                    .with_hint("default values are only allowed in named struct declarations (`struct Name { ... }`)")
                    .emit();
            }

            anon_fields.push(AnonymousField {
                name: f.name,
                ty: f_ty,
            });
        }

        anon_fields
    }

    fn make_builtin_optional_type(&mut self, inner_ty: TypeId, _span: Span) -> TypeId {
        let some = self.ctx.intern("Some");
        let none = self.ctx.intern("None");
        self.ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: None,
                builtin: Some(BuiltinAnonymousEnumKind::Optional),
                variants: vec![
                    AnonymousVariant {
                        name: some,
                        name_span: Span::default(),
                        payload_ty: Some(inner_ty),
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: none,
                        name_span: Span::default(),
                        payload_ty: None,
                        explicit_value: None,
                    },
                ],
            }))
    }

    fn make_builtin_result_type(&mut self, ok_ty: TypeId, err_ty: TypeId) -> TypeId {
        let ok = self.ctx.intern("Ok");
        let err = self.ctx.intern("Err");
        self.ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: None,
                builtin: Some(BuiltinAnonymousEnumKind::Result),
                variants: vec![
                    AnonymousVariant {
                        name: ok,
                        name_span: Span::default(),
                        payload_ty: Some(ok_ty),
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: err,
                        name_span: Span::default(),
                        payload_ty: Some(err_ty),
                        explicit_value: None,
                    },
                ],
            }))
    }

    fn check_duplicate_anon_fields(
        &mut self,
        fields: &[AnonymousField],
        span: Span,
        kind_name: &str,
    ) {
        for i in 1..fields.len() {
            if fields[i - 1].name == fields[i].name {
                let name_str = self.ctx.resolve(fields[i].name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!("duplicate field `{}` in {}", name_str, kind_name),
                    )
                    .emit();
            }
        }
    }

    fn check_duplicate_anon_variants(&mut self, variants: &[AnonymousVariant], span: Span) {
        let mut sorted = variants.to_vec();
        sorted.sort_by_key(|variant| variant.name);
        for i in 1..sorted.len() {
            if sorted[i - 1].name == sorted[i].name {
                let name_str = self.ctx.resolve(sorted[i].name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!("duplicate variant `{}` in anonymous enum", name_str),
                    )
                    .emit();
            }
        }
    }

    pub(super) fn resolve_builtin_primitive(&mut self, name: &str) -> Option<TypeId> {
        let scalar = match name {
            "void" => Some(TypeId::VOID),
            "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8),
            "i16" => Some(TypeId::I16),
            "i32" => Some(TypeId::I32),
            "i64" => Some(TypeId::I64),
            "i128" => Some(TypeId::I128),
            "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8),
            "u16" => Some(TypeId::U16),
            "u32" => Some(TypeId::U32),
            "u64" => Some(TypeId::U64),
            "u128" => Some(TypeId::U128),
            "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32),
            "f64" => Some(TypeId::F64),
            "never" => Some(TypeId::NEVER),
            _ => None,
        };

        if scalar.is_some() {
            return scalar;
        }

        self.parse_builtin_simd(name)
    }

    fn parse_builtin_simd(&mut self, name: &str) -> Option<TypeId> {
        let (base, lanes) = name.rsplit_once('x')?;
        let lanes: u16 = lanes.parse().ok()?;
        if lanes == 0 {
            return None;
        }

        let elem = match base {
            "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8),
            "i16" => Some(TypeId::I16),
            "i32" => Some(TypeId::I32),
            "i64" => Some(TypeId::I64),
            "i128" => Some(TypeId::I128),
            "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8),
            "u16" => Some(TypeId::U16),
            "u32" => Some(TypeId::U32),
            "u64" => Some(TypeId::U64),
            "u128" => Some(TypeId::U128),
            "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32),
            "f64" => Some(TypeId::F64),
            _ => None,
        }?;

        Some(
            self.ctx
                .type_registry
                .intern(TypeKind::Simd { elem, lanes }),
        )
    }
}
