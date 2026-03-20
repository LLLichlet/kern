use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::Expr;
use kernc_mast::*;
use kernc_sema::def::Def;
use kernc_sema::scope::SymbolKind;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_identifier(&mut self, name: SymbolId) -> MastExprKind {
        // 优先检查是否是顶层全局变量
        if let Some(&mono_id) = self.global_symbol_map.get(&name) {
            return MastExprKind::GlobalRef(mono_id);
        }

        // 其次检查是否是局部作用域内的 static 变量
        for scope in self.local_statics.iter().rev() {
            if let Some(&mono_id) = scope.get(&name) {
                return MastExprKind::GlobalRef(mono_id);
            }
        }

        // 走到这里，说明它要么是函数，要么是普通的局部变量(let/param)
        if let Some(info) = self.ctx.scopes.resolve(name).cloned() {
            match info.kind {
                SymbolKind::Function => {
                    let fn_def_id = info.def_id.unwrap();
                    let mono_id = self.instantiate_function(fn_def_id, &[]);
                    MastExprKind::FuncRef(mono_id)
                }
                _ => MastExprKind::Var(name),
            }
        } else {
            MastExprKind::Var(name)
        }
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
            unreachable!(
                "Attempted to access method `{}` without calling it. Bound Methods are not supported.",
                self.ctx.resolve(field)
            );
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

        // 如果是 Enum 变体的访问 (例如 Color.Red)，复用枚举字面量提取逻辑
        if let TypeKind::Enum(def_id, _) = self.ctx.type_registry.get(norm_base) {
            if let Def::Enum(_) = &self.ctx.defs[def_id.0 as usize] {
                return self.lower_enum_literal(field, expr_ty);
            }
        }

        let field_idx = self.get_field_index(base_ty, field);
        let struct_def_info =
            if let TypeKind::Def(def_id, gen_args) = self.ctx.type_registry.get(norm_base) {
                Some((*def_id, gen_args.clone()))
            } else {
                None
            };

        let struct_id = if let Some((def_id, gen_args)) = struct_def_info {
            self.instantiate_struct(def_id, &gen_args)
        } else {
            unreachable!("Field access on non-struct type");
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

    pub(crate) fn get_field_index(&self, struct_ty: TypeId, field_name: SymbolId) -> usize {
        let norm = self.ctx.type_registry.normalize(struct_ty);
        if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm) {
            if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                return s.fields.iter().position(|f| f.name == field_name).unwrap();
            } else if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
                return u.fields.iter().position(|f| f.name == field_name).unwrap();
            }
        }
        0
    }
}
