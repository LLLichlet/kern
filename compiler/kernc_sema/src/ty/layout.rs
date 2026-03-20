use crate::SemaContext;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_ast as ast;
use kernc_utils::SymbolId;
use std::collections::HashMap;

pub struct LayoutEngine<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
}

impl<'a, 'ctx> LayoutEngine<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn compute_type_align(&mut self, ty: TypeId) -> u64 {
        self.compute_type_align_inner(ty, 0)
    }

    fn compute_type_align_inner(&mut self, ty: TypeId, depth: usize) -> u64 {
        if depth > 100 {
            return 1;
        }

        let norm = self.ctx.type_registry.normalize(ty);
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } | TypeKind::Function { .. } => {
                self.ctx.sess.target.pointer_size
            }
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => self.ctx.sess.target.pointer_size,

            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.compute_type_align_inner(elem, depth + 1)
            }

            TypeKind::Def(def_id, generic_args) | TypeKind::Enum(def_id, generic_args) => {
                self.compute_def_align(def_id, &generic_args, depth)
            }
            TypeKind::Primitive(PrimitiveType::Never) | TypeKind::Error => 1,
            TypeKind::Primitive(p) => self.primitive_align(p),

            // TODO: 如果遇到 TypeVar 等其他推导中的未知类型，兜底对齐为 1
            _ => 1,
        }
    }

    pub fn compute_type_size(&mut self, ty: TypeId) -> u64 {
        self.compute_type_size_inner(ty, 0)
    }

    fn compute_type_size_inner(&mut self, ty: TypeId, depth: usize) -> u64 {
        if depth > 100 {
            return 0;
        }

        let norm = self.ctx.type_registry.normalize(ty);
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } | TypeKind::Function { .. } => {
                self.ctx.sess.target.pointer_size
            }
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => {
                self.ctx.sess.target.pointer_size * 2
            }

            // 处理定长数组，ArrayInfer 属于未知长度，暂时返回 0
            TypeKind::Array { elem, len, .. } => {
                self.compute_type_size_inner(elem, depth + 1) * len
            }
            // TODO:
            TypeKind::ArrayInfer { .. } => 0,

            TypeKind::Def(def_id, generic_args) | TypeKind::Enum(def_id, generic_args) => {
                self.compute_def_size(def_id, &generic_args, depth)
            }
            TypeKind::Error | TypeKind::Primitive(PrimitiveType::Never) => 0,
            TypeKind::Primitive(p) => self.primitive_size(p),

            // TODO: 兜底推导中未解出的 TypeVar 为 0
            _ => 0,
        }
    }

    fn align_to(offset: u64, align: u64) -> u64 {
        (offset + align - 1) & !(align - 1)
    }

    fn primitive_align(&self, p: PrimitiveType) -> u64 {
        use PrimitiveType::*;
        match p {
            I8 | U8 | Bool => 1,
            I16 | U16 => 2,
            I32 | U32 | F32 => 4,
            I64 | U64 | F64 => 8,
            ISize | USize => self.ctx.sess.target.pointer_size,
            I128 | U128 => 16,
            _ => 1,
        }
    }

    fn primitive_size(&self, p: PrimitiveType) -> u64 {
        use PrimitiveType::*;
        match p {
            I8 | U8 | Bool => 1,
            I16 | U16 => 2,
            I32 | U32 | F32 => 4,
            I64 | U64 | F64 => 8,
            ISize | USize => self.ctx.sess.target.pointer_size,
            I128 | U128 => 16,
            _ => 0,
        }
    }

    fn compute_def_align(&mut self, def_id: DefId, generic_args: &[TypeId], depth: usize) -> u64 {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        match def {
            Def::Struct(s) => {
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut max_align = 1;
                for field in &s.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let align = self.compute_type_align_inner(f_ty, depth + 1);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            Def::Union(u) => {
                let map = self.prepare_generic_subst(&u.generics, generic_args);
                let mut max_align = 1;
                for field in &u.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let align = self.compute_type_align_inner(f_ty, depth + 1);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            Def::Enum(a) => {
                let tag_ty = a.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx
                        .node_types
                        .get(&bt.id)
                        .copied()
                        .unwrap_or(TypeId::U32)
                });
                let mut max_align = self.compute_type_align_inner(tag_ty, depth + 1);

                let map = self.prepare_generic_subst(&a.generics, generic_args);
                for v in &a.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, depth + 1);
                        if align > max_align {
                            max_align = align;
                        }
                    }
                }
                max_align
            }
            _ => 1,
        }
    }

    fn compute_def_size(&mut self, def_id: DefId, generic_args: &[TypeId], depth: usize) -> u64 {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        match def {
            Def::Struct(s) => {
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut offset = 0;
                let mut max_align = 1;

                for field in &s.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, depth + 1);
                    let f_size = self.compute_type_size_inner(f_ty, depth + 1);

                    if f_align > max_align {
                        max_align = f_align;
                    }
                    offset = Self::align_to(offset, f_align);
                    offset += f_size;
                }
                Self::align_to(offset, max_align)
            }
            Def::Union(u) => {
                let map = self.prepare_generic_subst(&u.generics, generic_args);
                let mut max_size = 0;
                let mut max_align = 1;

                for field in &u.fields {
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, depth + 1);
                    let f_size = self.compute_type_size_inner(f_ty, depth + 1);

                    if f_align > max_align {
                        max_align = f_align;
                    }
                    if f_size > max_size {
                        max_size = f_size;
                    }
                }
                Self::align_to(max_size, max_align)
            }
            Def::Enum(a) => {
                // Enum Size = align_to(TagSize, MaxAlign) + align_to(MaxPayloadSize, MaxAlign)
                // TODO: (简化版的 C 布局，实际以 target data_layout 为准)
                let tag_ty = a.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx
                        .node_types
                        .get(&bt.id)
                        .copied()
                        .unwrap_or(TypeId::U32)
                });
                let mut max_align = self.compute_type_align_inner(tag_ty, depth + 1);
                let tag_size = self.compute_type_size_inner(tag_ty, depth + 1);

                let map = self.prepare_generic_subst(&a.generics, generic_args);
                let mut max_payload_size = 0;

                for v in &a.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, depth + 1);
                        let size = self.compute_type_size_inner(p_ty, depth + 1);
                        if align > max_align {
                            max_align = align;
                        }
                        if size > max_payload_size {
                            max_payload_size = size;
                        }
                    }
                }

                let mut offset = tag_size;
                offset = Self::align_to(offset, max_align);
                offset += max_payload_size;
                Self::align_to(offset, max_align)
            }
            _ => 0,
        }
    }

    fn prepare_generic_subst(
        &self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
    ) -> HashMap<SymbolId, TypeId> {
        let mut map = HashMap::new();
        if !generics.is_empty() && !args.is_empty() {
            for (i, param) in generics.iter().enumerate() {
                map.insert(param.name, args[i]);
            }
        }
        map
    }

    fn resolve_field_type(
        &mut self,
        type_node: &ast::TypeNode,
        map: &HashMap<SymbolId, TypeId>,
    ) -> TypeId {
        let mut f_ty = self
            .ctx
            .node_types
            .get(&type_node.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        if !map.is_empty() {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, map);
            f_ty = subst.substitute(f_ty);
        }
        f_ty
    }
}
