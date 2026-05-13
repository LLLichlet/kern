use super::Lowerer;
use crate::ActiveFunctionInstantiation;
use kernc_ast as ast;
use kernc_mast::*;
use kernc_mono::MonoId;
use kernc_sema::LayoutEngine;
use kernc_sema::checker::{ConstEvaluator, ConstValue};
use kernc_sema::def::{Def, DefId, GlobalDef};
use kernc_sema::ty::{GenericArg, TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

const MAX_ACTIVE_FUNCTION_INSTANTIATION_DEPTH: usize = 128;
const MAX_PENDING_FUNCTION_SPECIALIZATIONS: usize = 1024;

#[derive(Debug, Clone, Copy)]
enum SpecializationLimit {
    ActiveDepth,
    PendingQueue,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn aligned_union_storage_size(size: u64, align: u64) -> usize {
        let size = size.max(1);
        let align = align.max(1);
        let remainder = size % align;
        let padded = if remainder == 0 {
            size
        } else {
            size + (align - remainder)
        };
        padded as usize
    }

    pub(crate) fn drain_pending_function_instantiations(&mut self) {
        while self.next_pending_function_instantiation < self.pending_function_instantiations.len()
        {
            let pending = self.pending_function_instantiations
                [self.next_pending_function_instantiation]
                .clone();
            self.next_pending_function_instantiation += 1;
            let saved_active = std::mem::replace(
                &mut self.active_function_instantiations,
                pending.lineage.clone(),
            );
            self.measure_phase("  lower_instantiate_function", |this| {
                this.finish_function_instantiation(
                    pending.def_id,
                    &pending.args,
                    pending.id,
                    pending.request_span,
                )
            });
            self.active_function_instantiations = saved_active;
        }
        self.pending_function_instantiations.clear();
        self.next_pending_function_instantiation = 0;
    }

    pub(crate) fn lower_const_value_expr(
        &mut self,
        value: &ConstValue,
        ty: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        match value {
            ConstValue::Int(v) => Some(MastExpr::new(ty, MastExprKind::Integer(*v as u128), span)),
            ConstValue::Float(f) => Some(MastExpr::new(ty, MastExprKind::Float(*f), span)),
            ConstValue::Bool(b) => Some(MastExpr::new(ty, MastExprKind::Bool(*b), span)),
            ConstValue::String(s) => {
                let kind = match self
                    .ctx
                    .type_registry
                    .get(self.ctx.type_registry.normalize(ty))
                {
                    TypeKind::Array { .. } | TypeKind::ArrayInfer { .. } => {
                        self.lower_string_literal_array(s, span)
                    }
                    _ => MastExprKind::StringLiteral(s.clone()),
                };
                Some(MastExpr::new(ty, kind, span))
            }
            ConstValue::Array(items) => {
                let elem_ty = match self
                    .ctx
                    .type_registry
                    .get(self.ctx.type_registry.normalize(ty))
                {
                    TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => *elem,
                    _ => return None,
                };

                let mut elems = Vec::with_capacity(items.len());
                for item in items {
                    elems.push(self.lower_const_value_expr(item, elem_ty, span)?);
                }

                Some(MastExpr::new(ty, MastExprKind::ArrayInit(elems), span))
            }
            ConstValue::Struct(fields) => self.lower_const_struct_value_expr(fields, ty, span),
            ConstValue::Enum { tag, payload } => {
                self.lower_const_enum_value_expr(*tag, payload.as_deref(), ty, span)
            }
            ConstValue::Undef => Some(MastExpr::new(ty, MastExprKind::Undef, span)),
            ConstValue::Void => Some(MastExpr::new(ty, MastExprKind::Undef, span)),
            _ => None,
        }
    }

    fn lower_const_struct_value_expr(
        &mut self,
        fields: &HashMap<SymbolId, ConstValue>,
        ty: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let norm_ty = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm_ty).clone() {
            TypeKind::Def(def_id, gen_args) => {
                let Def::Struct(def) = self.ctx.defs.get(def_id.0 as usize)?.clone() else {
                    return None;
                };
                let struct_id = self.instantiate_struct(def_id, &gen_args);

                let mut subst_map = HashMap::new();
                for (param, arg) in def.generics.iter().zip(gen_args.iter()) {
                    subst_map.insert(param.name, *arg);
                }

                let mut ast_ordered_exprs = Vec::with_capacity(def.fields.len());
                for field in &def.fields {
                    let raw_ty = self
                        .ctx
                        .node_type(field.type_node.id)
                        .unwrap_or(TypeId::ERROR);
                    let field_ty = self.substitute_type_with_map(raw_ty, &subst_map);
                    let value = fields.get(&field.name)?;
                    ast_ordered_exprs.push(self.lower_const_value_expr(value, field_ty, span)?);
                }

                let (_, physical_to_ast) = self.cached_named_struct_mapping(def_id, &gen_args);
                let mut physical_ordered_exprs = Vec::with_capacity(def.fields.len());
                for &ast_idx in &physical_to_ast {
                    physical_ordered_exprs.push(ast_ordered_exprs[ast_idx].clone());
                }

                Some(MastExpr::new(
                    ty,
                    MastExprKind::StructInit {
                        struct_id,
                        fields: physical_ordered_exprs,
                    },
                    span,
                ))
            }
            TypeKind::AnonymousStruct(is_extern, anon_fields) => {
                let struct_id = self.instantiate_anon_struct(norm_ty);
                let mut ast_ordered_exprs = Vec::with_capacity(anon_fields.len());
                for field in &anon_fields {
                    let value = fields.get(&field.name)?;
                    ast_ordered_exprs.push(self.lower_const_value_expr(value, field.ty, span)?);
                }

                let (_, physical_to_ast) =
                    self.cached_anon_struct_mapping(norm_ty, is_extern, &anon_fields);
                let mut physical_ordered_exprs = Vec::with_capacity(anon_fields.len());
                for &ast_idx in &physical_to_ast {
                    physical_ordered_exprs.push(ast_ordered_exprs[ast_idx].clone());
                }

                Some(MastExpr::new(
                    ty,
                    MastExprKind::StructInit {
                        struct_id,
                        fields: physical_ordered_exprs,
                    },
                    span,
                ))
            }
            _ => None,
        }
    }

    pub(crate) fn lower_const_global_value_expr(
        &mut self,
        def_id: DefId,
        span: Span,
    ) -> Option<MastExpr> {
        let const_expr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
            g.value.clone()?
        } else {
            return None;
        };
        let ty = self.ctx.node_type(const_expr.id).unwrap_or(TypeId::ERROR);

        let prev_scope = self.ctx.scopes.current_scope_id();
        if let Some(owner_scope) = self.global_owner_scope(def_id) {
            self.ctx.scopes.set_current_scope(owner_scope);
        }

        let lowered = {
            let mut ce = ConstEvaluator::new(self.ctx);
            ce.eval_inner(&const_expr, 0)
                .ok()
                .and_then(|value| self.lower_const_value_expr(&value, ty, span))
        };

        if let Some(prev_scope) = prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }

        lowered
    }

    fn lower_const_enum_value_expr(
        &mut self,
        tag: i128,
        payload: Option<&ConstValue>,
        ty: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let norm_ty = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm_ty).clone() {
            TypeKind::AnonymousEnum(enum_def) => {
                if enum_def
                    .variants
                    .iter()
                    .all(|variant| variant.payload_ty.is_none())
                {
                    return Some(MastExpr::new(ty, MastExprKind::Integer(tag as u128), span));
                }

                let mut current_tag = 0i128;
                let mut payload_ty = TypeId::VOID;
                let mut found = false;
                for variant in &enum_def.variants {
                    if let Some(value) = variant.explicit_value {
                        current_tag = value;
                    }
                    if current_tag == tag {
                        payload_ty = variant.payload_ty.unwrap_or(TypeId::VOID);
                        found = true;
                        break;
                    }
                    current_tag += 1;
                }
                if !found {
                    return None;
                }

                let payload_expr = if payload_ty == TypeId::VOID {
                    MastExpr::new(TypeId::VOID, MastExprKind::Undef, span)
                } else {
                    self.lower_const_value_expr(payload?, payload_ty, span)?
                };
                let mono_id = self.instantiate_anon_enum(norm_ty);
                Some(MastExpr::new(
                    ty,
                    MastExprKind::DataInit {
                        data_struct_id: mono_id,
                        tag_value: tag as u128,
                        payload: Box::new(payload_expr),
                    },
                    span,
                ))
            }
            TypeKind::Enum(def_id, gen_args) => {
                let Def::Enum(def) = self.ctx.defs.get(def_id.0 as usize)?.clone() else {
                    return None;
                };
                if self.is_pure_enum(&def) {
                    self.record_pure_enum_tag_ty(def_id, &gen_args);
                    return Some(MastExpr::new(ty, MastExprKind::Integer(tag as u128), span));
                }

                let mut generic_map = HashMap::new();
                for (param, arg) in def.generics.iter().zip(gen_args.iter()) {
                    generic_map.insert(param.name, *arg);
                }

                let mut current_tag = 0i128;
                let mut payload_ty = TypeId::VOID;
                let mut found = false;
                for variant in &def.variants {
                    if let Some(value_expr) = &variant.value {
                        let mut ce = ConstEvaluator::new(self.ctx);
                        if let Ok(value) = ce.eval_math(value_expr) {
                            current_tag = value;
                        }
                    }
                    if current_tag == tag {
                        if let Some(payload_ast) = &variant.payload_type {
                            let raw_payload_ty = self.ctx.node_type(payload_ast.id)?;
                            payload_ty =
                                self.substitute_type_with_map(raw_payload_ty, &generic_map);
                        }
                        found = true;
                        break;
                    }
                    current_tag += 1;
                }
                if !found {
                    return None;
                }

                let payload_expr = if payload_ty == TypeId::VOID {
                    MastExpr::new(TypeId::VOID, MastExprKind::Undef, span)
                } else {
                    self.lower_const_value_expr(payload?, payload_ty, span)?
                };
                let mono_id = self.instantiate_data(def_id, &gen_args);
                Some(MastExpr::new(
                    ty,
                    MastExprKind::DataInit {
                        data_struct_id: mono_id,
                        tag_value: tag as u128,
                        payload: Box::new(payload_expr),
                    },
                    span,
                ))
            }
            _ => None,
        }
    }

    fn placeholder_function(&mut self, id: MonoId, name: String) {
        if self.module.functions.iter().any(|func| func.id == id) {
            return;
        }

        self.module.functions.push(MastFunction {
            id,
            name,
            span: Span::default(),
            linkage: MastLinkage::Internal,
            params: vec![],
            ret_ty: TypeId::VOID,
            body: Some(MastBlock {
                stmts: vec![MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Trap,
                    Span::default(),
                ))],
                result: None,
                defers: vec![],
            }),
            is_extern: false,
            is_variadic: false,
            inline_hint: MastInlineHint::None,
            attributes: vec![],
        });
    }

    fn placeholder_struct(&mut self, id: MonoId, name: String, is_union: bool) {
        if self.module.structs.iter().any(|strukt| strukt.id == id) {
            return;
        }

        self.module.structs.push(MastStruct {
            id,
            name,
            fields: vec![],
            is_extern: false,
            is_union,
            largest_field_idx: 0,
            union_size: if is_union { 1 } else { 0 },
            union_align: 1,
            attributes: vec![],
        });
    }

    fn placeholder_data_structs(
        &mut self,
        wrapper_id: MonoId,
        payload_union_id: MonoId,
        name: &str,
    ) {
        self.placeholder_struct(payload_union_id, format!("{}_payload", name), true);
        self.placeholder_struct(wrapper_id, name.to_string(), false);
    }

    pub(crate) fn build_generic_subst_map(
        &mut self,
        owner_kind: &str,
        owner_name: &str,
        params: &[ast::GenericParam],
        args: &[GenericArg],
    ) -> Option<HashMap<SymbolId, GenericArg>> {
        if params.len() != args.len() {
            self.ctx
                .struct_error(
                    Span::default(),
                    format!(
                        "generic argument count mismatch for {} `{}`: expected {}, got {}",
                        owner_kind,
                        owner_name,
                        params.len(),
                        args.len()
                    ),
                )
                .emit();
            return None;
        }

        let mut subst_map = HashMap::new();
        for (param, arg) in params.iter().zip(args.iter().copied()) {
            subst_map.insert(param.name, arg);
        }
        Some(subst_map)
    }

    pub(crate) fn instantiate_function_at(
        &mut self,
        def_id: DefId,
        args: &[GenericArg],
        request_span: Span,
    ) -> MonoId {
        let key = self.measure_phase("  lower_mono_fn_key", |_this| (def_id, args.to_vec()));
        if let Some(id) = self.measure_phase("  lower_mono_fn_lookup", |this| {
            this.mono_cache.get(&key).copied()
        }) {
            self.cache_stats.mono_function_hits += 1;
            return id;
        }
        let pending_outstanding = self
            .pending_function_instantiations
            .len()
            .saturating_sub(self.next_pending_function_instantiation);
        if pending_outstanding >= MAX_PENDING_FUNCTION_SPECIALIZATIONS {
            self.drain_pending_function_instantiations();
        }
        if let Some(limit) = self.recursive_specialization_limit() {
            self.cache_stats.mono_function_misses += 1;
            let id = self.new_mono_id();
            self.mono_cache.insert(key, id);
            self.placeholder_function(id, self.ctx.get_export_name_for_generic_args(def_id, args));
            self.emit_specialization_limit_diagnostic(limit, def_id, args, request_span);
            return id;
        }
        if let Some(ancestor_index) = self.detect_infinite_polymorphic_recursion(def_id, args) {
            self.cache_stats.mono_function_misses += 1;
            let id = self.new_mono_id();
            self.mono_cache.insert(key, id);
            self.placeholder_function(id, self.ctx.get_export_name_for_generic_args(def_id, args));
            self.emit_infinite_polymorphic_recursion_diagnostic(
                ancestor_index,
                def_id,
                args,
                request_span,
            );
            return id;
        }
        self.cache_stats.mono_function_misses += 1;
        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);
        self.pending_function_instantiations
            .push(crate::PendingFunctionInstantiation {
                def_id,
                args: args.to_vec(),
                id,
                request_span,
                lineage: self.active_function_instantiations.clone(),
            });
        id
    }

    fn recursive_specialization_limit(&self) -> Option<SpecializationLimit> {
        if self.active_function_instantiations.len() >= MAX_ACTIVE_FUNCTION_INSTANTIATION_DEPTH {
            return Some(SpecializationLimit::ActiveDepth);
        }

        let pending_outstanding = self
            .pending_function_instantiations
            .len()
            .saturating_sub(self.next_pending_function_instantiation);
        if pending_outstanding >= MAX_PENDING_FUNCTION_SPECIALIZATIONS {
            return Some(SpecializationLimit::PendingQueue);
        }

        None
    }

    fn finish_function_instantiation(
        &mut self,
        def_id: DefId,
        args: &[GenericArg],
        id: MonoId,
        request_span: Span,
    ) -> MonoId {
        let Some(def_ptr) = self
            .ctx
            .defs
            .get(def_id.0 as usize)
            .and_then(|def| match def {
                Def::Function(function) => Some(std::ptr::from_ref(function)),
                _ => None,
            })
        else {
            self.ctx.emit_ice(
                Span::default(),
                format!("Kern ICE (Lowering): DefId {} is not a Function!", def_id.0),
            );
            self.placeholder_function(id, format!("__ice_fn_{}", id.0));
            return id;
        };
        // Safety: lowering reads semantic definition storage but does not mutate or reorder
        // `ctx.defs`, so the raw pointer stays valid for the duration of this instantiation.
        let fn_name = unsafe { self.ctx.resolve((*def_ptr).name).to_string() };

        let Some((subst_map, mangled_name, mast_params, conc_ret)) =
            self.measure_phase("    lower_fn_signature", |this| {
                let def = unsafe { &*def_ptr };
                let subst_map =
                    this.build_generic_subst_map("function", &fn_name, &def.generics, args)?;
                let mangled_name = this.ctx.get_export_name_for_generic_args(def_id, args);

                let (raw_sig_params, raw_ret) = def
                    .resolved_sig
                    .and_then(|sig| match this.ctx.type_registry.get(sig).clone() {
                        TypeKind::Function { params, ret, .. } => Some((params, ret)),
                        _ => None,
                    })
                    .unwrap_or_else(|| (Vec::new(), TypeId::VOID));
                let use_resolved_sig_params = raw_sig_params.len() == def.params.len();
                if def.resolved_sig.is_some() && !use_resolved_sig_params {
                    this.ctx.emit_ice(
                        def.name_span,
                        format!(
                            "Kern ICE (Lowering): resolved signature for function `{}` contains {} parameters, but the AST definition contains {}.",
                            fn_name,
                            raw_sig_params.len(),
                            def.params.len()
                        ),
                    );
                }

                let mut mast_params = Vec::with_capacity(def.params.len());
                for (idx, p) in def.params.iter().enumerate() {
                    let raw_ty = raw_sig_params.get(idx).copied().filter(|_| use_resolved_sig_params).unwrap_or_else(|| {
                        this.ctx.node_type(p.type_node.id)
                            .unwrap_or(TypeId::ERROR)
                    });
                    let conc_ty = this.substitute_type_with_map(raw_ty, &subst_map);
                    this.track_pure_enum_repr_in_type(conc_ty);
                    mast_params.push(MastParam {
                        name: p.pattern.name,
                        ty: conc_ty,
                        is_mut: p.pattern.is_mut,
                    });
                }

                let conc_ret = this.substitute_type_with_map(raw_ret, &subst_map);
                this.track_pure_enum_repr_in_type(conc_ret);

                Some((subst_map, mangled_name, mast_params, conc_ret))
            })
        else {
            self.placeholder_function(id, format!("__ice_fn_{}", id.0));
            return id;
        };

        self.active_function_instantiations
            .push(ActiveFunctionInstantiation {
                def_id,
                args: args
                    .iter()
                    .map(|arg| match *arg {
                        GenericArg::Type(ty) => {
                            GenericArg::Type(self.ctx.type_registry.normalize(ty))
                        }
                        GenericArg::Const(value) => GenericArg::Const(value),
                    })
                    .collect(),
                request_span,
            });

        let (
            saved_local_types,
            saved_local_forwardings,
            saved_local_value_forwardings,
            saved_defer_stack,
            saved_loop_frames,
            saved_local_statics,
        ) = self.measure_phase("    lower_fn_scope_setup", |this| {
            let saved_local_types = std::mem::take(&mut this.local_types);
            let saved_local_forwardings = std::mem::take(&mut this.local_forwardings);
            let saved_local_value_forwardings = std::mem::take(&mut this.local_value_forwardings);
            let saved_defer_stack = std::mem::take(&mut this.defer_stack);
            let saved_loop_frames = std::mem::take(&mut this.loop_frames);
            let saved_local_statics = std::mem::take(&mut this.local_statics);

            this.local_types.push(std::collections::HashMap::new());
            this.local_forwardings
                .push(std::collections::HashMap::new());
            this.local_value_forwardings
                .push(std::collections::HashMap::new());
            for p in &mast_params {
                if let Some(scope) = this.local_types.last_mut() {
                    scope.insert(p.name, (p.ty, p.is_mut));
                } else {
                    this.ctx.emit_ice(
                        Span::default(),
                        "Kern ICE (Lowering): Missing local type scope while instantiating a function.",
                    );
                    break;
                }
            }

            (
                saved_local_types,
                saved_local_forwardings,
                saved_local_value_forwardings,
                saved_defer_stack,
                saved_loop_frames,
                saved_local_statics,
            )
        });

        let body = self.measure_phase("    lower_fn_body", |this| {
            let def = unsafe { &*def_ptr };
            if this.function_requires_runtime_body(def) {
                let prev_scope = this.ctx.scopes.current_scope_id();
                let saved_owner = this.current_owner_def_id.replace(def_id);
                if let Some(owner_scope) = this.function_owner_scope(def) {
                    this.ctx.scopes.set_current_scope(owner_scope);
                }
                this.current_return_types.push(conc_ret);

                let body = def
                    .body
                    .as_ref()
                    .map(|body_expr| this.lower_block_as_body(body_expr, &subst_map, conc_ret));

                this.current_return_types.pop();
                if let Some(prev_scope) = prev_scope {
                    this.ctx.scopes.set_current_scope(prev_scope);
                }
                this.current_owner_def_id = saved_owner;

                body
            } else {
                None
            }
        });

        self.active_function_instantiations.pop();

        self.measure_phase("    lower_fn_scope_restore", |this| {
            this.local_types.pop();
            this.local_forwardings.pop();
            this.local_value_forwardings.pop();

            this.local_types = saved_local_types;
            this.local_forwardings = saved_local_forwardings;
            this.local_value_forwardings = saved_local_value_forwardings;
            this.defer_stack = saved_defer_stack;
            this.loop_frames = saved_loop_frames;
            this.local_statics = saved_local_statics;
        });

        let uses_odr_linkage = {
            let def = unsafe { &*def_ptr };
            !def.generics.is_empty() && body.is_some() && !def.is_extern
        };

        self.measure_phase("    lower_fn_finalize", |this| {
            let def = unsafe { &*def_ptr };
            let mast_fn = MastFunction {
                id,
                name: mangled_name,
                span: def.name_span,
                linkage: this.lowered_function_linkage(
                    def.vis,
                    def.is_extern,
                    &def.attributes,
                    uses_odr_linkage,
                ),
                params: mast_params,
                ret_ty: conc_ret,
                body,
                is_extern: def.is_extern,
                is_variadic: def.is_variadic,
                inline_hint: this.lowered_inline_hint(&def.attributes),
                attributes: this.extract_meta_items(&def.attributes),
            };

            this.module.functions.push(mast_fn);
        });
        id
    }

    fn detect_infinite_polymorphic_recursion(
        &self,
        def_id: DefId,
        args: &[GenericArg],
    ) -> Option<usize> {
        let normalized_args: Vec<_> = args
            .iter()
            .map(|arg| match *arg {
                GenericArg::Type(ty) => GenericArg::Type(self.ctx.type_registry.normalize(ty)),
                GenericArg::Const(value) => GenericArg::Const(value),
            })
            .collect();

        self.active_function_instantiations
            .iter()
            .enumerate()
            .find_map(|(index, frame)| {
                (frame.def_id == def_id
                    && self.function_instantiation_strictly_grows(&frame.args, &normalized_args))
                .then_some(index)
            })
    }

    fn function_instantiation_strictly_grows(
        &self,
        previous_args: &[GenericArg],
        next_args: &[GenericArg],
    ) -> bool {
        if previous_args == next_args {
            return false;
        }

        let previous_args = kernc_sema::ty::erase_non_type_generic_args(previous_args);
        let next_args = kernc_sema::ty::erase_non_type_generic_args(next_args);

        previous_args.iter().any(|previous_arg| {
            next_args.iter().any(|next_arg| {
                matches!(
                    self.type_containment(*previous_arg, *next_arg),
                    TypeContainment::Proper
                )
            })
        })
    }

    fn type_containment(&self, needle: TypeId, haystack: TypeId) -> TypeContainment {
        let needle = self.ctx.type_registry.normalize(needle);
        let haystack = self.ctx.type_registry.normalize(haystack);
        if needle == haystack {
            return TypeContainment::Equal;
        }

        let contains_child = |child: TypeId, this: &Self| match this.type_containment(needle, child)
        {
            TypeContainment::None => TypeContainment::None,
            TypeContainment::Equal | TypeContainment::Proper => TypeContainment::Proper,
        };

        match self.ctx.type_registry.get(haystack).clone() {
            TypeKind::Primitive(..)
            | TypeKind::Param(..)
            | TypeKind::Error
            | TypeKind::Module(..)
            | TypeKind::TypeVar(..) => TypeContainment::None,
            TypeKind::Simd { elem, .. }
            | TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Array { elem, .. }
            | TypeKind::ArrayInfer { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => contains_child(elem, self),
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::FnDef(_, args)
            | TypeKind::Associated(_, args) => args
                .into_iter()
                .filter_map(|arg| arg.as_type())
                .map(|arg| contains_child(arg, self))
                .find(|containment| *containment != TypeContainment::None)
                .unwrap_or(TypeContainment::None),
            TypeKind::TraitObject(_, args, assoc_bindings) => args
                .into_iter()
                .filter_map(|arg| arg.as_type())
                .map(|arg| contains_child(arg, self))
                .chain(
                    assoc_bindings
                        .into_iter()
                        .map(|(_, assoc_ty)| contains_child(assoc_ty, self)),
                )
                .find(|containment| *containment != TypeContainment::None)
                .unwrap_or(TypeContainment::None),
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => std::iter::once(contains_child(target, self))
                .chain(
                    trait_args
                        .into_iter()
                        .filter_map(|arg| arg.as_type())
                        .map(|arg| contains_child(arg, self)),
                )
                .chain(
                    assoc_args
                        .into_iter()
                        .filter_map(|arg| arg.as_type())
                        .map(|arg| contains_child(arg, self)),
                )
                .find(|containment| *containment != TypeContainment::None)
                .unwrap_or(TypeContainment::None),
            TypeKind::ClosureInterface { params, ret } | TypeKind::Function { params, ret, .. } => {
                params
                    .into_iter()
                    .map(|param| contains_child(param, self))
                    .chain(std::iter::once(contains_child(ret, self)))
                    .find(|containment| *containment != TypeContainment::None)
                    .unwrap_or(TypeContainment::None)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => captures
                .into_iter()
                .map(|capture| contains_child(capture, self))
                .chain(params.into_iter().map(|param| contains_child(param, self)))
                .chain(std::iter::once(contains_child(ret, self)))
                .find(|containment| *containment != TypeContainment::None)
                .unwrap_or(TypeContainment::None),
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .map(|field| contains_child(field.ty, self))
                .find(|containment| *containment != TypeContainment::None)
                .unwrap_or(TypeContainment::None),
            TypeKind::AnonymousEnum(enum_def) => enum_def
                .backing_ty
                .into_iter()
                .map(|ty| contains_child(ty, self))
                .chain(
                    enum_def
                        .variants
                        .into_iter()
                        .filter_map(|variant| variant.payload_ty)
                        .map(|payload_ty| contains_child(payload_ty, self)),
                )
                .find(|containment| *containment != TypeContainment::None)
                .unwrap_or(TypeContainment::None),
        }
    }

    fn emit_infinite_polymorphic_recursion_diagnostic(
        &mut self,
        ancestor_index: usize,
        def_id: DefId,
        args: &[GenericArg],
        request_span: Span,
    ) {
        let current_display = self.function_instantiation_display(def_id, args);
        let function_name = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(function) => self.ctx.resolve(function.name).to_string(),
            _ => current_display.clone(),
        };
        let mut chain = self.active_function_instantiations[ancestor_index..]
            .iter()
            .map(|frame| self.function_instantiation_display(frame.def_id, &frame.args))
            .collect::<Vec<_>>();
        chain.push(current_display.clone());
        let ancestor_frame = &self.active_function_instantiations[ancestor_index];
        let ancestor_label = (ancestor_frame.request_span != Span::default()).then(|| {
            (
                ancestor_frame.request_span,
                format!(
                    "instantiation of `{}` entered the recursive chain here",
                    self.function_instantiation_display(
                        ancestor_frame.def_id,
                        &ancestor_frame.args
                    )
                ),
            )
        });
        let declaration_label = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(function) if function.name_span != Span::default() => Some((
                function.name_span,
                format!("generic function `{}` is declared here", function_name),
            )),
            _ => None,
        };

        let mut diag = self
            .ctx
            .struct_error(
                request_span,
                format!(
                    "generic function `{}` recursively requires infinitely many specializations",
                    function_name
                ),
            )
            .with_hint(
                "Kern monomorphizes generic functions; this recursive instantiation grows the type arguments instead of reusing an existing specialization",
            )
            .with_hint(format!("instantiation chain: {}", chain.join(" -> ")))
            .with_hint(
                "rewrite the recursion so recursive calls reuse the same specialization, or move the type growth into data instead of the call graph",
            );

        if let Some((span, label)) = ancestor_label {
            diag = diag.with_span_label(span, label);
        }

        if let Some((span, label)) = declaration_label {
            diag = diag.with_span_label(span, label);
        }

        diag.emit();
    }

    fn emit_specialization_limit_diagnostic(
        &mut self,
        limit: SpecializationLimit,
        def_id: DefId,
        args: &[GenericArg],
        request_span: Span,
    ) {
        let current_display = self.function_instantiation_display(def_id, args);
        let function_name = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(function) => self.ctx.resolve(function.name).to_string(),
            _ => current_display.clone(),
        };
        let chain = self
            .active_function_instantiations
            .iter()
            .map(|frame| self.function_instantiation_display(frame.def_id, &frame.args))
            .chain(std::iter::once(current_display.clone()))
            .collect::<Vec<_>>();
        let recursive_anchor_label =
            self.active_function_instantiations
                .first()
                .and_then(|frame| {
                    (frame.request_span != Span::default()).then(|| {
                        (
                            frame.request_span,
                            self.function_instantiation_display(frame.def_id, &frame.args),
                        )
                    })
                });
        let const_instability_hint = self.const_specialization_instability_hint(def_id, args);

        let mut diag = match limit {
            SpecializationLimit::ActiveDepth => self
                .ctx
                .struct_error(
                    request_span,
                    format!(
                        "generic function `{}` exceeded the recursive specialization depth limit",
                        function_name
                    ),
                )
                .with_hint(format!(
                    "Kern aborted lowering after {} active recursive instantiations to avoid runaway monomorphization",
                    MAX_ACTIVE_FUNCTION_INSTANTIATION_DEPTH
                ))
                .with_hint(format!("instantiation chain: {}", chain.join(" -> "))),
            SpecializationLimit::PendingQueue => self
                .ctx
                .struct_error(
                    request_span,
                    format!(
                        "generic function `{}` exceeded the specialization work queue limit",
                        function_name
                    ),
                )
                .with_hint(format!(
                    "Kern aborted lowering after queuing {} pending generic function specializations to avoid runaway monomorphization",
                    MAX_PENDING_FUNCTION_SPECIALIZATIONS
                ))
                .with_hint(format!("instantiation chain: {}", chain.join(" -> "))),
        };

        if let Some((ancestor_span, ancestor_display)) = recursive_anchor_label {
            diag = diag.with_span_label(
                ancestor_span,
                format!(
                    "instantiation of `{}` entered the runaway specialization chain here",
                    ancestor_display
                ),
            );
        }

        if let Some(hint) = const_instability_hint {
            diag = diag.with_hint(hint);
        }

        diag.with_hint(
            "rewrite the recursion so it reuses existing specializations, or move the growing compile-time state into data instead of the call graph",
        )
        .emit();
    }

    fn const_specialization_instability_hint(
        &self,
        def_id: DefId,
        args: &[GenericArg],
    ) -> Option<String> {
        let function_name = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(function) => self.ctx.resolve(function.name).to_string(),
            _ => self.function_instantiation_display(def_id, args),
        };
        let current_display = self.function_instantiation_display(def_id, args);
        self.active_function_instantiations
            .iter()
            .rev()
            .find(|frame| {
                frame.def_id == def_id
                    && self.function_instantiation_has_stable_type_args_but_shifted_consts(
                        &frame.args, args,
                    )
            })
            .map(|frame| {
                let previous_display =
                    self.function_instantiation_display(frame.def_id, &frame.args);
                format!(
                    "const generic arguments do not stabilize across recursive calls; `{}` keeps forcing new specializations such as `{}` -> `{}`",
                    function_name,
                    previous_display,
                    current_display,
                )
            })
    }

    fn function_instantiation_has_stable_type_args_but_shifted_consts(
        &self,
        previous_args: &[GenericArg],
        next_args: &[GenericArg],
    ) -> bool {
        if previous_args.len() != next_args.len() {
            return false;
        }

        let mut changed_const = false;
        for (previous, next) in previous_args.iter().zip(next_args.iter()) {
            match (*previous, *next) {
                (GenericArg::Type(previous_ty), GenericArg::Type(next_ty)) => {
                    if self.ctx.type_registry.normalize(previous_ty)
                        != self.ctx.type_registry.normalize(next_ty)
                    {
                        return false;
                    }
                }
                (GenericArg::Const(previous_const), GenericArg::Const(next_const)) => {
                    if previous_const != next_const {
                        changed_const = true;
                    }
                }
                _ => return false,
            }
        }

        changed_const
    }

    fn function_instantiation_display(&self, def_id: DefId, args: &[GenericArg]) -> String {
        let name = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(function) => self.ctx.resolve(function.name).to_string(),
            other => other
                .name()
                .map(|name| self.ctx.resolve(name).to_string())
                .unwrap_or_else(|| format!("def#{}", def_id.0)),
        };

        if args.is_empty() {
            return name;
        }

        let rendered_args = args
            .iter()
            .map(|arg| match *arg {
                GenericArg::Type(ty) => self.ctx.ty_to_string(ty),
                GenericArg::Const(value) => value.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{name}[{rendered_args}]")
    }

    pub(crate) fn instantiate_struct(&mut self, def_id: DefId, args: &[GenericArg]) -> MonoId {
        let key = self.measure_phase("  lower_mono_struct_key", |_this| (def_id, args.to_vec()));
        if let Some(id) = self.measure_phase("  lower_mono_struct_lookup", |this| {
            this.mono_cache.get(&key).copied()
        }) {
            self.cache_stats.mono_struct_hits += 1;
            return id;
        }
        self.cache_stats.mono_struct_misses += 1;
        self.measure_phase("  lower_instantiate_struct", |this| {
            let id = this.new_mono_id();
            this.mono_cache.insert(key, id);

            if let Def::Union(_) = &this.ctx.defs[def_id.0 as usize] {
                return this.instantiate_union(def_id, args, id);
            }

            let def = if let Def::Struct(s) = &this.ctx.defs[def_id.0 as usize] {
                s.clone()
            } else {
                this.ctx.emit_ice(
                    Span::default(),
                    format!("Kern ICE (Lowering): DefId {} is not a Struct!", def_id.0),
                );
                this.placeholder_struct(id, format!("__ice_struct_{}", id.0), false);
                return id;
            };

            let mangled_name = this.ctx.get_export_name_for_generic_args(def_id, args);
            let Some(subst_map) =
                this.build_generic_subst_map("struct", &mangled_name, &def.generics, args)
            else {
                this.placeholder_struct(id, format!("__ice_struct_{}", id.0), false);
                return id;
            };

            let (_, physical_to_ast) = this.cached_named_struct_mapping(def_id, args);

            let mut mast_fields = Vec::with_capacity(def.fields.len());

            for &ast_idx in &physical_to_ast {
                let f = &def.fields[ast_idx];
                let raw_ty = this.ctx.node_type(f.type_node.id).unwrap_or(TypeId::ERROR);
                let conc_ty = this.substitute_type_with_map(raw_ty, &subst_map);
                this.track_pure_enum_repr_in_type(conc_ty);
                mast_fields.push(MastField {
                    name: f.name,
                    ty: conc_ty,
                });
            }

            this.module.structs.push(MastStruct {
                id,
                name: mangled_name,
                fields: mast_fields,
                is_extern: def.is_extern,
                is_union: false,
                largest_field_idx: 0,
                union_size: 0,
                union_align: 1,
                attributes: this.extract_meta_items(&def.attributes),
            });

            id
        })
    }

    pub(crate) fn instantiate_anon_struct(&mut self, norm_ty: TypeId) -> MonoId {
        if let Some(&id) = self.anon_struct_cache.get(&norm_ty) {
            return id;
        }

        let id = self.new_mono_id();
        self.anon_struct_cache.insert(norm_ty, id);

        let (is_extern, fields) = if let TypeKind::AnonymousStruct(ext, f) =
            self.ctx.type_registry.get(norm_ty).clone()
        {
            (ext, f)
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): Expected AnonymousStruct, found {:?}",
                    self.ctx.type_registry.get(norm_ty)
                ),
            );
            self.placeholder_struct(id, format!("__ice_anon_struct_{}", id.0), false);
            return id;
        };

        let (_, physical_to_ast) = self.cached_anon_struct_mapping(norm_ty, is_extern, &fields);

        let mut mast_fields = Vec::with_capacity(fields.len());

        for &ast_idx in &physical_to_ast {
            let f = &fields[ast_idx];
            self.track_pure_enum_repr_in_type(f.ty);
            mast_fields.push(MastField {
                name: f.name,
                ty: f.ty,
            });
        }

        let mangled_name = self.ctx.mangle_type(norm_ty);

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern,
            is_union: false,
            largest_field_idx: 0,
            union_size: 0,
            union_align: 1,
            attributes: vec![],
        });

        id
    }

    pub(crate) fn instantiate_anon_union(&mut self, norm_ty: TypeId) -> MonoId {
        if let Some(&id) = self.anon_union_cache.get(&norm_ty) {
            return id;
        }

        let id = self.new_mono_id();
        self.anon_union_cache.insert(norm_ty, id);

        let (is_extern, fields) =
            if let TypeKind::AnonymousUnion(ext, f) = self.ctx.type_registry.get(norm_ty).clone() {
                (ext, f)
            } else {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Lowering): Expected AnonymousUnion, found {:?}",
                        self.ctx.type_registry.get(norm_ty)
                    ),
                );
                self.placeholder_struct(id, format!("__ice_anon_union_{}", id.0), true);
                return id;
            };

        let mut mast_fields = Vec::new();
        let mut max_size = 0;
        let mut max_align = 1;
        let mut largest_field_idx = 0;

        for (idx, field) in fields.iter().enumerate() {
            self.track_pure_enum_repr_in_type(field.ty);
            mast_fields.push(MastField {
                name: field.name,
                ty: field.ty,
            });

            let mut layout = LayoutEngine::new(self.ctx);
            let size = layout.compute_type_size(field.ty);
            let align = layout.compute_type_align(field.ty);
            if size > max_size {
                max_size = size;
                largest_field_idx = idx;
            }
            max_align = max_align.max(align);
        }

        self.module.structs.push(MastStruct {
            id,
            name: self.ctx.mangle_type(norm_ty),
            fields: mast_fields,
            is_extern,
            is_union: true,
            largest_field_idx,
            union_size: Self::aligned_union_storage_size(max_size, max_align),
            union_align: max_align.max(1) as usize,
            attributes: vec![],
        });

        id
    }

    pub(crate) fn instantiate_anon_enum(&mut self, norm_ty: TypeId) -> MonoId {
        if let Some(&id) = self.anon_enum_cache.get(&norm_ty) {
            return id;
        }

        let wrapper_id = self.new_mono_id();
        let payload_union_id = self.new_mono_id();
        self.anon_enum_cache.insert(norm_ty, wrapper_id);
        self.adt_union_map.insert(wrapper_id, payload_union_id);

        let enum_def = if let TypeKind::AnonymousEnum(enum_def) =
            self.ctx.type_registry.get(norm_ty).clone()
        {
            enum_def
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): Expected AnonymousEnum, found {:?}",
                    self.ctx.type_registry.get(norm_ty)
                ),
            );
            self.placeholder_data_structs(
                wrapper_id,
                payload_union_id,
                &format!("__ice_anon_enum_{}", wrapper_id.0),
            );
            return wrapper_id;
        };

        let mut union_fields = Vec::new();
        let mut largest_idx = 0;
        let mut max_size = 0;
        let mut max_align = 1;
        for variant in &enum_def.variants {
            let field_ty = variant.payload_ty.unwrap_or(TypeId::VOID);
            self.track_pure_enum_repr_in_type(field_ty);

            union_fields.push(MastField {
                name: variant.name,
                ty: field_ty,
            });
        }

        let mut layout = LayoutEngine::new(self.ctx);
        for (idx, field) in union_fields.iter().enumerate() {
            let field_ty = field.ty;
            if field_ty != TypeId::VOID && field_ty != TypeId::ERROR {
                let size = layout.compute_type_size(field_ty);
                let align = layout.compute_type_align(field_ty);
                if size > max_size {
                    max_size = size;
                    largest_idx = idx;
                }
                max_align = max_align.max(align);
            }
        }

        let mangled_name = self.ctx.mangle_type(norm_ty);

        self.module.structs.push(MastStruct {
            id: payload_union_id,
            name: format!("{}_payload", mangled_name),
            fields: union_fields,
            is_extern: false,
            is_union: true,
            largest_field_idx: largest_idx,
            union_size: Self::aligned_union_storage_size(max_size, max_align),
            union_align: max_align.max(1) as usize,
            attributes: vec![],
        });

        let payload_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::AnonymousEnumPayload(norm_ty));
        let tag_ty = enum_def.backing_ty.unwrap_or(TypeId::U32);

        self.module.structs.push(MastStruct {
            id: wrapper_id,
            name: mangled_name,
            fields: vec![
                MastField {
                    name: self.ctx.intern("__tag"),
                    ty: tag_ty,
                },
                MastField {
                    name: self.ctx.intern("__payload"),
                    ty: payload_ty,
                },
            ],
            is_extern: false,
            is_union: false,
            largest_field_idx: 0,
            union_size: 0,
            union_align: 1,
            attributes: vec![],
        });

        wrapper_id
    }

    pub(crate) fn instantiate_union(
        &mut self,
        def_id: DefId,
        args: &[GenericArg],
        id: MonoId,
    ) -> MonoId {
        let def = if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
            u.clone()
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!("Kern ICE (Lowering): DefId {} is not a Union!", def_id.0),
            );
            self.placeholder_struct(id, format!("__ice_union_{}", id.0), true);
            return id;
        };

        let mangled_name = self.ctx.get_export_name_for_generic_args(def_id, args);
        let Some(subst_map) =
            self.build_generic_subst_map("union", &mangled_name, &def.generics, args)
        else {
            self.placeholder_struct(id, format!("__ice_union_{}", id.0), true);
            return id;
        };

        let mut mast_fields = Vec::new();
        let mut max_size = 0;
        let mut max_align = 1;
        let mut largest_field_idx = 0;

        for f in &def.fields {
            let raw_ty = self.ctx.node_type(f.type_node.id).unwrap_or(TypeId::ERROR);
            let conc_ty = self.substitute_type_with_map(raw_ty, &subst_map);
            self.track_pure_enum_repr_in_type(conc_ty);
            mast_fields.push(MastField {
                name: f.name,
                ty: conc_ty,
            });
        }

        let mut layout = LayoutEngine::new(self.ctx);
        for (idx, field) in mast_fields.iter().enumerate() {
            let size = layout.compute_type_size(field.ty);
            let align = layout.compute_type_align(field.ty);

            if size > max_size {
                max_size = size;
                largest_field_idx = idx;
            }
            max_align = max_align.max(align);
        }

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern: def.is_extern,
            is_union: true,
            largest_field_idx,
            union_size: Self::aligned_union_storage_size(max_size, max_align),
            union_align: max_align.max(1) as usize,
            attributes: vec![],
        });
        id
    }

    pub(crate) fn instantiate_data(&mut self, def_id: DefId, args: &[GenericArg]) -> MonoId {
        let key = self.measure_phase("  lower_mono_data_key", |_this| (def_id, args.to_vec()));
        if let Some(id) = self.measure_phase("  lower_mono_data_lookup", |this| {
            this.mono_cache.get(&key).copied()
        }) {
            self.cache_stats.mono_data_hits += 1;
            return id;
        }
        self.cache_stats.mono_data_misses += 1;
        self.measure_phase("  lower_instantiate_data", |this| {
            let wrapper_id = this.new_mono_id();
            let payload_union_id = this.new_mono_id();
            this.mono_cache.insert(key, wrapper_id);
            this.adt_union_map.insert(wrapper_id, payload_union_id);

            let def = if let Def::Enum(a) = &this.ctx.defs[def_id.0 as usize] {
                a.clone()
            } else {
                this.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Lowering): DefId {} is not an Enum (Data)! ",
                        def_id.0
                    ),
                );
                this.placeholder_data_structs(
                    wrapper_id,
                    payload_union_id,
                    &format!("__ice_enum_{}", wrapper_id.0),
                );
                return wrapper_id;
            };

            let mangled_name = this.ctx.get_export_name_for_generic_args(def_id, args);
            let Some(subst_map) =
                this.build_generic_subst_map("enum", &mangled_name, &def.generics, args)
            else {
                this.placeholder_data_structs(
                    wrapper_id,
                    payload_union_id,
                    &format!("__ice_enum_{}", wrapper_id.0),
                );
                return wrapper_id;
            };

            let mut union_fields = Vec::new();
            let mut largest_idx = 0;
            let mut max_size = 0;
            let mut max_align = 1;

            for variant in &def.variants {
                let field_ty = if let Some(payload_ast) = &variant.payload_type {
                    let raw_ty = this.ctx.node_type(payload_ast.id).unwrap_or(TypeId::ERROR);
                    this.substitute_type_with_map(raw_ty, &subst_map)
                } else {
                    TypeId::VOID
                };
                this.track_pure_enum_repr_in_type(field_ty);

                union_fields.push(MastField {
                    name: variant.name,
                    ty: field_ty,
                });
            }

            let mut layout = LayoutEngine::new(this.ctx);
            for (idx, field) in union_fields.iter().enumerate() {
                let field_ty = field.ty;
                if field_ty != TypeId::VOID && field_ty != TypeId::ERROR {
                    let size = layout.compute_type_size(field_ty);
                    let align = layout.compute_type_align(field_ty);

                    if size > max_size {
                        max_size = size;
                        largest_idx = idx;
                    }
                    max_align = max_align.max(align);
                }
            }

            this.module.structs.push(MastStruct {
                id: payload_union_id,
                name: format!("{}_payload", mangled_name),
                fields: union_fields,
                is_extern: false,
                is_union: true,
                largest_field_idx: largest_idx,
                union_size: Self::aligned_union_storage_size(max_size, max_align),
                union_align: max_align.max(1) as usize,
                attributes: vec![],
            });

            let tag_ty = if let Some(bt) = &def.backing_type {
                let raw_tag_ty = this.ctx.node_type(bt.id).unwrap_or(TypeId::U32);
                this.substitute_type_with_map(raw_tag_ty, &subst_map)
            } else {
                TypeId::U32
            };

            let union_ty = this
                .ctx
                .type_registry
                .intern(TypeKind::EnumPayload(def_id, args.to_vec()));

            this.module.structs.push(MastStruct {
                id: wrapper_id,
                name: mangled_name,
                fields: vec![
                    MastField {
                        name: this.ctx.intern("__tag"),
                        ty: tag_ty,
                    },
                    MastField {
                        name: this.ctx.intern("__payload"),
                        ty: union_ty,
                    },
                ],
                is_extern: false,
                is_union: false,
                largest_field_idx: 0,
                union_size: 0,
                union_align: 1,
                attributes: vec![],
            });

            wrapper_id
        })
    }

    pub(crate) fn lower_global(&mut self, g: &GlobalDef) {
        let id = match self.global_map.get(&g.id) {
            Some(&id) => id,
            None => {
                let name = self.ctx.resolve(g.name);
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    format!("Kern ICE (Lowering): Global MonoId for `{}` missing.", name),
                );
                let placeholder = self.new_mono_id();
                self.global_map.insert(g.id, placeholder);
                placeholder
            }
        };

        let ty = g
            .value
            .as_ref()
            .and_then(|value| self.ctx.node_type(value.id))
            .or_else(|| {
                g.type_node
                    .as_ref()
                    .and_then(|type_node| self.ctx.node_type(type_node.id))
            })
            .unwrap_or(TypeId::ERROR);
        self.track_pure_enum_repr_in_type(ty);
        let is_mut = g.is_mut;

        // Perform constant folding.
        let init = if !g.is_extern {
            let Some(value) = g.value.as_ref() else {
                self.ctx.emit_ice(
                    g.span,
                    "Kern ICE (Lowering): non-extern global missing initializer.",
                );
                return;
            };
            let prev_scope = self.ctx.scopes.current_scope_id();
            let saved_owner = self.current_owner_def_id.replace(g.id);
            if let Some(owner_scope) = self.global_owner_scope(g.id) {
                self.ctx.scopes.set_current_scope(owner_scope);
            }

            let folded = {
                let mut ce = ConstEvaluator::new(self.ctx);
                if let Ok(val) = ce.eval_inner(value, 0) {
                    self.lower_const_value_expr(&val, ty, g.span)
                        .or_else(|| Some(self.lower_expr(value, &HashMap::new(), Some(ty))))
                } else {
                    Some(self.lower_expr(value, &HashMap::new(), Some(ty)))
                }
            };

            if let Some(prev_scope) = prev_scope {
                self.ctx.scopes.set_current_scope(prev_scope);
            }
            self.current_owner_def_id = saved_owner;

            folded
        } else {
            None
        };

        if !g.is_static && !g.is_extern {
            self.ctx.emit_ice(
                g.span,
                format!(
                    "Kern ICE (Lowering): const `{}` reached global lowering instead of being inlined.",
                    self.ctx.resolve(g.name)
                ),
            );
            return;
        }

        self.module.globals.push(MastGlobal {
            id,
            name: self.ctx.get_export_name(g.id, &[]),
            span: g.span,
            linkage: self.lowered_global_linkage(g.vis, g.is_extern, &g.attributes),
            ty,
            is_mut,
            init,
            is_extern: g.is_extern,
            attributes: self.extract_meta_items(&g.attributes),
        });
    }

    pub(crate) fn ensure_global_lowered(&mut self, def_id: DefId) {
        if self.module.globals.iter().any(|global| {
            self.global_map
                .get(&def_id)
                .is_some_and(|mono_id| *mono_id == global.id)
        }) {
            return;
        }

        let Some(Def::Global(global)) = self.ctx.defs.get(def_id.0 as usize).cloned() else {
            return;
        };
        self.lower_global(&global);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeContainment {
    None,
    Equal,
    Proper,
}
