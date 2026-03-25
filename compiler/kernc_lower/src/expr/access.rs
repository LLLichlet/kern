use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::Expr;
use kernc_mast::*;
use kernc_sema::checker::{ConstEvaluator, ConstValue};
use kernc_sema::def::Def;
use kernc_sema::scope::SymbolKind;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_identifier(&mut self, name: SymbolId) -> MastExprKind {
        // 常量内联
        if let Some(info) = self.ctx.scopes.resolve(name).cloned() {
            if info.kind == SymbolKind::Const {
                if let Some(def_id) = info.def_id {
                    let const_expr_opt = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                        Some(g.value.clone())
                    } else {
                        None
                    };

                    if let Some(const_expr) = const_expr_opt {
                        let mut ce = ConstEvaluator::new(self.ctx);
                        if let Ok(val) = ce.eval_inner(&const_expr, 0) {
                            match val {
                                ConstValue::Int(v) => return MastExprKind::Integer(v as u128),
                                ConstValue::Float(f) => return MastExprKind::Float(f),
                                ConstValue::Bool(b) => return MastExprKind::Bool(b),
                                ConstValue::String(s) => return MastExprKind::StringLiteral(s),
                                _ => {
                                    let inlined_mast = self.lower_expr(&const_expr, &std::collections::HashMap::new(), None);
                                    return inlined_mast.kind;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 优先检查是否是顶层全局变量 (静态数组、全局字符串等)
        if let Some(&mono_id) = self.global_symbol_map.get(&name) {
            return MastExprKind::GlobalRef(mono_id);
        }

        // 其次检查是否是局部作用域内的 static 变量
        for scope in self.local_statics.iter().rev() {
            if let Some(&mono_id) = scope.get(&name) {
                return MastExprKind::GlobalRef(mono_id);
            }
        }

        // 因为在外层 (mod.rs) 已经通过 node_types 拦截了 FnDef (函数引用)
        // 走到这里，它一定是一个普通的局部变量 (let 绑定或函数参数)
        MastExprKind::Var(name)
    }

    pub(crate) fn lower_field_access(
        &mut self,
        lhs: &Expr,
        field: SymbolId,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let expr_ty = self
            .ctx
            .node_types
            .get(&lhs.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_expr = self.ctx.type_registry.normalize(expr_ty);

        if matches!(
            self.ctx.type_registry.get(norm_expr),
            TypeKind::FnDef(..) | TypeKind::Function { .. }
        ) {
            self.ctx.emit_ice(
                span,
                format!("Attempted to access method `{}` without calling it. Bound Methods are not supported in Kern.", self.ctx.resolve(field))
            );
            unreachable!()
        }

        let l = self.lower_expr(lhs, subst_map, None);
        let mut base_ty = l.ty;
        let mut deref_expr = l.clone();

        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                    deref_expr =
                        MastExpr::new(base_ty, MastExprKind::Deref(Box::new(deref_expr)), span);
                }
                _ => break,
            }
        }

        let norm_base = self.ctx.type_registry.normalize(base_ty);

        if let TypeKind::Enum(def_id, _) = self.ctx.type_registry.get(norm_base) {
            if let Def::Enum(_) = &self.ctx.defs[def_id.0 as usize] {
                return self.lower_enum_literal(field, expr_ty);
            }
        }

        if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(norm_base).clone() {
            let mod_def = match &self.ctx.defs[mod_def_id.0 as usize] {
                Def::Module(m) => m,
                _ => {
                    self.ctx.emit_ice(
                        span,
                        "Kern ICE (Lowering): Expected Module Def, found something else.",
                    );
                    unreachable!()
                }
            };

            let target_info = match self.ctx.scopes.resolve_in(mod_def.scope_id, field) {
                Some(info) => info,
                None => {
                    self.ctx.emit_ice(span, format!("Kern ICE (Lowering): Module field `{}` is undefined. Sema should have caught this.", self.ctx.resolve(field)));
                    unreachable!()
                }
            };

            match target_info.kind {
                SymbolKind::Module => {
                    return MastExprKind::Var(field);
                }
                SymbolKind::Const => {
                    if let Some(def_id) = target_info.def_id {
                        let const_expr_opt =
                            if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                                Some(g.value.clone())
                            } else {
                                None
                            };

                        if let Some(const_expr) = const_expr_opt {
                            let mut ce = ConstEvaluator::new(self.ctx);
                            if let Ok(val) = ce.eval_inner(&const_expr, 0) {
                                match val {
                                    ConstValue::Int(v) => return MastExprKind::Integer(v as u128),
                                    ConstValue::Float(f) => return MastExprKind::Float(f),
                                    ConstValue::Bool(b) => return MastExprKind::Bool(b),
                                    ConstValue::String(s) => return MastExprKind::StringLiteral(s),
                                    
                                    _ => {
                                        let inlined_mast = self.lower_expr(&const_expr, &HashMap::new(), None);
                                        return inlined_mast.kind;
                                    }
                                }
                            }
                        }
                    }

                    // 如果无法内联（比如是个数组常量），从全局映射表获取
                    if let Some(&mono_id) = self.global_symbol_map.get(&field) {
                        return MastExprKind::GlobalRef(mono_id);
                    } else {
                        let field_name = self.ctx.resolve(field);
                        self.ctx.emit_ice(
                            span,
                            format!("Kern ICE (Lowering): Cross-module constant `{}` could not be inlined, and its global definition was not found. Phase 1 global collection failed.", field_name)
                        );
                        unreachable!()
                    }
                }
                SymbolKind::Static | SymbolKind::Function => {
                    if let Some(&mono_id) = self.global_symbol_map.get(&field) {
                        return MastExprKind::GlobalRef(mono_id);
                    } else if target_info.def_id.is_some() {
                        self.ctx.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Lowering): Symbol `{}` found but not instantiated.",
                                self.ctx.resolve(field)
                            ),
                        );
                        unreachable!()
                    } else {
                        self.ctx.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Lowering): Symbol `{}` lacks a def_id.",
                                self.ctx.resolve(field)
                            ),
                        );
                        unreachable!()
                    }
                }
                _ => {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): Unsupported symbol kind in module: {:?}",
                            target_info.kind
                        ),
                    );
                    unreachable!()
                }
            }
        }

        let field_idx = self.get_field_index(base_ty, field, span);
        let struct_def_info =
            if let TypeKind::Def(def_id, gen_args) = self.ctx.type_registry.get(norm_base) {
                Some((*def_id, gen_args.clone()))
            } else {
                None
            };

        let struct_id = if let Some((def_id, gen_args)) = struct_def_info {
            self.instantiate_struct(def_id, &gen_args)
        } else {
            self.ctx.emit_ice(
                span,
                format!("Kern ICE (Lowering): Attempted to access field `{}` on a non-struct/union/module type: {:?}", self.ctx.resolve(field), norm_base)
            );
            unreachable!()
        };

        MastExprKind::FieldAccess {
            lhs: Box::new(deref_expr),
            struct_id,
            field_idx,
        }
    }

    pub(crate) fn lower_index_access(
        &mut self,
        lhs: &Expr,
        index: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let l = self.lower_expr(lhs, subst_map, None);
        let idx = self.lower_expr(index, subst_map, Some(TypeId::USIZE));
        MastExprKind::IndexAccess {
            lhs: Box::new(l),
            index: Box::new(idx),
        }
    }

    pub(crate) fn get_field_index(
        &mut self,
        struct_ty: TypeId,
        field_name: SymbolId,
        span: Span,
    ) -> usize {
        let norm = self.ctx.type_registry.normalize(struct_ty);
        if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm) {
            if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                return match s.fields.iter().position(|f| f.name == field_name) {
                    Some(idx) => idx,
                    None => {
                        self.ctx.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Lowering): Field `{}` not found in struct `{}`",
                                self.ctx.resolve(field_name),
                                self.ctx.resolve(s.name)
                            ),
                        );
                        unreachable!()
                    }
                };
            } else if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
                return match u.fields.iter().position(|f| f.name == field_name) {
                    Some(idx) => idx,
                    None => {
                        self.ctx.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Lowering): Field `{}` not found in union `{}`",
                                self.ctx.resolve(field_name),
                                self.ctx.resolve(u.name)
                            ),
                        );
                        unreachable!()
                    }
                };
            }
        }
        0
    }
}
