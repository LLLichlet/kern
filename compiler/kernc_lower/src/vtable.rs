use super::Lowerer;
use kernc_mast::*;
use kernc_sema::def::{Def, DefId, ImplDef, TraitDef};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Span;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn get_or_create_vtable(&mut self, source_ty: TypeId, trait_ty: TypeId) -> MonoId {
        let norm_source = self.ctx.type_registry.normalize(source_ty);
        let norm_trait = self.ctx.type_registry.normalize(trait_ty);
        let key = (norm_source, norm_trait);
        if let Some(&id) = self.vtable_cache.get(&key) {
            return id;
        }

        let trait_def_id = match self.ctx.type_registry.get(trait_ty) {
            TypeKind::TraitObject(id, _) => *id,
            other => {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Lowering): Target must be a TraitObject, found: {:?}",
                        other
                    ),
                );
                unreachable!()
            }
        };

        let trait_def = if let Def::Trait(t) = &self.ctx.defs[trait_def_id.0 as usize] {
            t.clone()
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): DefId {} is not a Trait!",
                    trait_def_id.0
                ),
            );
            unreachable!()
        };

        let (base_source_ty, source_args) = self.resolve_vtable_source_base(source_ty);

        let impl_def = match self.find_matching_impl_block(base_source_ty, trait_def_id) {
            Some(def) => def,
            None => {
                let src_name = self.ctx.ty_to_string(base_source_ty);
                let trait_name = self.ctx.resolve(trait_def.name);
                self.ctx.emit_ice(
                    Span::default(),
                    format!("Kern ICE (Lowering): Impl block missing for cast `{} as {}`. Sema failed to enforce Trait bounding contract.", src_name, trait_name)
                );
                unreachable!()
            }
        };

        let vtable_id = self.new_mono_id();
        self.vtable_cache.insert(key, vtable_id);

        self.build_and_inject_vtable_global(
            vtable_id,
            source_ty,
            trait_ty,
            &trait_def,
            &impl_def,
            &source_args,
        );

        vtable_id
    }

    /// 辅助方法 1：剥离来源指针的所有包装，获取真正的具名底层类型和泛型实参
    pub(crate) fn resolve_vtable_source_base(&self, source_ty: TypeId) -> (TypeId, Vec<TypeId>) {
        let mut base_ty = source_ty;
        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                }
                _ => {
                    base_ty = norm;
                    break;
                }
            }
        }

        let source_args = match self.ctx.type_registry.get(base_ty) {
            TypeKind::Def(_, args) | TypeKind::Enum(_, args) => args.clone(),
            _ => Vec::new(),
        };

        (base_ty, source_args)
    }

    /// 辅助方法 2：在全局寻找 (SourceBaseType -> TargetTrait) 的确切 Impl 块实现
    pub(crate) fn find_matching_impl_block(
        &self,
        base_source_ty: TypeId,
        target_trait_id: DefId,
    ) -> Option<ImplDef> {
        // 辅助闭包：提取底层类型的 DefId，兼容 Struct/Union (Def) 和 Enum (Adt)
        let get_base_def_id = |ty: TypeId| -> Option<DefId> {
            let norm = self.ctx.type_registry.normalize(ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Def(id, _) | TypeKind::Enum(id, _) => Some(*id),
                _ => None,
            }
        };

        let src_base_id = get_base_def_id(base_source_ty);
        let norm_src_base = self.ctx.type_registry.normalize(base_source_ty);

        for &impl_id in &self.ctx.global_impls {
            if let Def::Impl(impl_def) = &self.ctx.defs[impl_id.0 as usize] {
                if let Some(impl_trait_node) = &impl_def.trait_type {
                    // 检查 Impl 块声称实现的 Trait
                    let i_trait_ty = self
                        .ctx
                        .node_types
                        .get(&impl_trait_node.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);

                    if let TypeKind::TraitObject(i_trait_id, _) =
                        self.ctx.type_registry.get(i_trait_ty)
                    {
                        if *i_trait_id == target_trait_id {
                            // 检查 Impl 块的目标类型是否匹配
                            let i_target_ty = self
                                .ctx
                                .node_types
                                .get(&impl_def.target_type.id)
                                .copied()
                                .unwrap_or(TypeId::ERROR);
                            let (i_target_base, _) = self.resolve_vtable_source_base(i_target_ty);

                            // 1. 如果两者都是聚合类型 (Struct/Union/Enum)，比对 DefId (忽略具体泛型参数)
                            if let (Some(target_id), Some(src_id)) =
                                (get_base_def_id(i_target_base), src_base_id)
                            {
                                if target_id == src_id {
                                    return Some(impl_def.clone());
                                }
                            }
                            // 2. 兜底比对：支持标量类型匹配 (例如 impl Trait for i32)
                            else if self.ctx.type_registry.normalize(i_target_base)
                                == norm_src_base
                            {
                                return Some(impl_def.clone());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// 辅助方法 3：将提取出来的方法单态化，组装成数组，并插入到全局 MastGlobal
    pub(crate) fn build_and_inject_vtable_global(
        &mut self,
        vtable_id: MonoId,
        source_ty: TypeId,
        actual_trait_ty: TypeId,
        trait_def: &TraitDef,
        impl_def: &ImplDef,
        source_args: &[TypeId],
    ) {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });
        let mut vtable_methods = Vec::new();

        // 遍历 Trait 定义的每一个方法契约
        for trait_method in &trait_def.methods {
            let mut method_mono_id = None;

            // 在 Impl 块中找到对应的实现
            for &m_id in &impl_def.methods {
                if let Def::Function(f) = &self.ctx.defs[m_id.0 as usize] {
                    if f.name == trait_method.name {
                        method_mono_id = Some(self.instantiate_function(m_id, source_args));
                        break;
                    }
                }
            }

            let m_id = match method_mono_id {
                Some(id) => id,
                None => {
                    let method_name = self.ctx.resolve(trait_method.name);
                    self.ctx.emit_ice(
                        Span::default(),
                        format!("Kern ICE (Lowering): Missing implementation for trait method `{}`. Sema failed to check trait completeness.", method_name)
                    );
                    unreachable!()
                }
            };

            // 将单态化后的函数指针强转为 *void 存入虚表
            vtable_methods.push(MastExpr::new(
                void_ptr_ty,
                MastExprKind::FuncRef(m_id),
                Span::default(),
            ));
        }

        let vtable_len = vtable_methods.len() as u64;
        let vtable_array_ty = self.ctx.type_registry.intern(TypeKind::Array {
            is_mut: false,
            elem: void_ptr_ty,
            len: vtable_len,
        });

        let vtable_init = MastExpr::new(
            vtable_array_ty,
            MastExprKind::ArrayInit(vtable_methods),
            Span::default(),
        );

        self.module.globals.push(MastGlobal {
            id: vtable_id,
            name: format!("__vtable_{}_{}", source_ty.0, actual_trait_ty.0),
            ty: vtable_array_ty,
            is_mut: false, // 虚表永远是静态不可变的只读数据
            init: Some(vtable_init),
            is_extern: false,
            attributes: vec![],
        });
    }
}
