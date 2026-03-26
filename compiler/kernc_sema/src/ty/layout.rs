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

    /// 计算具名结构体的物理排布
    /// 返回: (ast_to_physical, physical_to_ast)
    pub fn get_struct_mapping(&mut self, def_id: DefId, generic_args: &[TypeId], depth: usize) -> (Vec<usize>, Vec<usize>) {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        if let Def::Struct(s) = def {
            let map = self.prepare_generic_subst(&s.generics, generic_args);
            let mut field_metas = Vec::new();
            
            for (ast_idx, field) in s.fields.iter().enumerate() {
                let f_ty = self.resolve_field_type(&field.type_node, &map);
                let f_align = self.compute_type_align_inner(f_ty, depth + 1);
                let f_size = self.compute_type_size_inner(f_ty, depth + 1);
                field_metas.push((ast_idx, f_align, f_size));
            }

            // 除非标记了 extern，否则强行优化内存布局
            if !s.is_extern {
                field_metas.sort_by(|a, b| {
                    b.1.cmp(&a.1)               // 1. 对齐要求降序 (Alignment)
                       .then_with(|| b.2.cmp(&a.2)) // 2. 大小降序 (Size)
                       .then_with(|| a.0.cmp(&b.0)) // 3. AST 原始索引升序 (稳定排序)
                });
            }

            let mut ast_to_physical = vec![0; field_metas.len()];
            let mut physical_to_ast = vec![0; field_metas.len()];

            for (phys_idx, meta) in field_metas.into_iter().enumerate() {
                ast_to_physical[meta.0] = phys_idx;
                physical_to_ast[phys_idx] = meta.0;
            }

            (ast_to_physical, physical_to_ast)
        } else {
            unreachable!("Not a struct definition")
        }
    }

    /// 计算匿名结构体的物理排布 (匿名结构体永远被优化)
    pub fn get_anon_struct_mapping(
        &mut self,
        is_extern: bool,
        fields: &[crate::ty::AnonymousField],
        depth: usize,
    ) -> (Vec<usize>, Vec<usize>) {
        let mut field_metas = Vec::new();
        for (ast_idx, f) in fields.iter().enumerate() {
            let f_align = self.compute_type_align_inner(f.ty, depth + 1);
            let f_size = self.compute_type_size_inner(f.ty, depth + 1);
            field_metas.push((ast_idx, f_align, f_size));
        }

        // 只有原生的匿名结构体，才执行内存体积压缩优化
        if !is_extern {
            field_metas.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| b.2.cmp(&a.2))
                    .then_with(|| a.0.cmp(&b.0))
            });
        }

        let mut ast_to_physical = vec![0; field_metas.len()];
        let mut physical_to_ast = vec![0; field_metas.len()];

        for (phys_idx, meta) in field_metas.into_iter().enumerate() {
            ast_to_physical[meta.0] = phys_idx;
            physical_to_ast[phys_idx] = meta.0;
        }

        (ast_to_physical, physical_to_ast)
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
            TypeKind::AnonymousState { captures, .. } => {
                let mut max_align = 1;
                for cap_ty in captures {
                    let align = self.compute_type_align_inner(cap_ty, depth + 1);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            TypeKind::ClosureInterface { .. } => 1,
            TypeKind::Primitive(PrimitiveType::Never) | TypeKind::Error => 1,
            TypeKind::AnonymousStruct(_, fields) => {
                let mut max_align = 1;
                for f in fields {
                    let align = self.compute_type_align_inner(f.ty, depth + 1);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            TypeKind::Primitive(p) => self.primitive_align(p),

            TypeKind::EnumPayload(def_id, generic_args) => {
                let def = if let Def::Enum(a) = &self.ctx.defs[def_id.0 as usize] { a.clone() } else { unreachable!() };
                let map = self.prepare_generic_subst(&def.generics, &generic_args);
                let mut max_payload_align = 1;
                for v in &def.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, depth + 1);
                        if align > max_payload_align { max_payload_align = align; }
                    }
                }
                max_payload_align
            }
            
            TypeKind::TypeVar(_) => {
                self.ctx.emit_ice(kernc_utils::Span::default(), "Kern ICE (Layout): Attempted to compute memory alignment of an unresolved TypeVar.");
                1
            }
            _ => {
                self.ctx.emit_ice(kernc_utils::Span::default(), format!("Kern ICE (Layout): Attempted to compute alignment of an invalid or incomplete type: {:?}", kind));
                1
            }
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
            TypeKind::ArrayInfer { .. } => {
                self.ctx.emit_ice(kernc_utils::Span::default(), "Kern ICE (Layout): Cannot compute the size of an array with inferred length `[_]T`. It must be fully resolved.");
                0
            }

            TypeKind::Def(def_id, generic_args) | TypeKind::Enum(def_id, generic_args) => {
                self.compute_def_size(def_id, &generic_args, depth)
            }
            TypeKind::AnonymousState { captures, .. } => {
                let mut offset = 0;
                let mut max_align = 1;

                for cap_ty in captures {
                    let f_align = self.compute_type_align_inner(cap_ty, depth + 1);
                    let f_size = self.compute_type_size_inner(cap_ty, depth + 1);

                    if f_align > max_align {
                        max_align = f_align;
                    }
                    // 将当前偏移量对齐到该字段的要求
                    offset = Self::align_to(offset, f_align);
                    offset += f_size;
                }
                // 最后将结构体的总大小对齐到最大对齐要求 (Tail Padding)
                Self::align_to(offset, max_align)
            }
            TypeKind::ClosureInterface { .. } => 0,
            TypeKind::AnonymousStruct(is_extern, fields) => {
                let (_, physical_to_ast) = self.get_anon_struct_mapping(is_extern, &fields, depth);
                let mut offset = 0;
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let f = &fields[ast_idx];
                    let f_align = self.compute_type_align_inner(f.ty, depth + 1);
                    let f_size = self.compute_type_size_inner(f.ty, depth + 1);
                    if f_align > max_align { max_align = f_align; }
                    offset = Self::align_to(offset, f_align);
                    offset += f_size;
                }
                Self::align_to(offset, max_align)
            }
            TypeKind::Error | TypeKind::Primitive(PrimitiveType::Never) => 0,
            TypeKind::Primitive(p) => self.primitive_size(p),

            TypeKind::EnumPayload(def_id, generic_args) => {
                let def = if let Def::Enum(a) = &self.ctx.defs[def_id.0 as usize] {
                    a.clone()
                } else {
                    unreachable!()
                };
                let map = self.prepare_generic_subst(&def.generics, &generic_args);
                let mut max_payload_size = 0;
                for v in &def.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let size = self.compute_type_size_inner(p_ty, depth + 1);
                        if size > max_payload_size { max_payload_size = size; }
                    }
                }
                max_payload_size
            }

            TypeKind::TypeVar(_) => {
                self.ctx.emit_ice(kernc_utils::Span::default(), "Kern ICE (Layout): Cannot compute the size of an unresolved TypeVar.");
                0
            }
            _ => {
                self.ctx.emit_ice(kernc_utils::Span::default(), format!("Kern ICE (Layout): Cannot compute the size of an invalid or incomplete type: {:?}", kind));
                0
            }
        }
    }

    fn align_to(offset: u64, align: u64) -> u64 {
        (offset + align - 1) & !(align - 1)
    }

    fn primitive_align(&self, p: PrimitiveType) -> u64 {
        use PrimitiveType::*;
        match p {
            Void => 1,
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
            Void => 0,
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
                let (_, physical_to_ast) = self.get_struct_mapping(def_id, generic_args, depth);
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut offset = 0;
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let field = &s.fields[ast_idx];
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, depth + 1);
                    let f_size = self.compute_type_size_inner(f_ty, depth + 1);

                    if f_align > max_align { max_align = f_align; }
                    offset = Self::align_to(offset, f_align);
                    offset += f_size;
                }
                Self::align_to(offset, max_align)
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
                let (_, physical_to_ast) = self.get_struct_mapping(def_id, generic_args, depth);
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut offset = 0;
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let field = &s.fields[ast_idx];
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, depth + 1);
                    let f_size = self.compute_type_size_inner(f_ty, depth + 1);

                    if f_align > max_align { max_align = f_align; }
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
                // C-ABI Tagged Union 布局: struct { TagType tag; union { ... } payload; }
                let tag_ty = a.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx
                        .node_types
                        .get(&bt.id)
                        .copied()
                        .unwrap_or(TypeId::U32)
                });
                
                let tag_align = self.compute_type_align_inner(tag_ty, depth + 1);
                let tag_size = self.compute_type_size_inner(tag_ty, depth + 1);

                let map = self.prepare_generic_subst(&a.generics, generic_args);
                
                // 追踪 Payload Union 的最大尺寸和最大对齐要求
                let mut max_payload_size = 0;
                let mut max_payload_align = 1;

                for v in &a.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, depth + 1);
                        let size = self.compute_type_size_inner(p_ty, depth + 1);
                        
                        if align > max_payload_align {
                            max_payload_align = align;
                        }
                        if size > max_payload_size {
                            max_payload_size = size;
                        }
                    }
                }

                // 1. 整体 Enum 的对齐要求
                let enum_align = tag_align.max(max_payload_align);

                // 如果是纯枚举 (无 payload)，其大小直接就是 Tag 对齐后的大小
                if max_payload_size == 0 {
                    return Self::align_to(tag_size, enum_align);
                }

                // 2. 计算 Payload Union 的内存偏移起点 (受 Union 自身对齐要求约束)
                let payload_offset = Self::align_to(tag_size, max_payload_align);

                // 3. 计算总大小并应用尾部填充 (Tail Padding)
                let total_size = payload_offset + max_payload_size;
                Self::align_to(total_size, enum_align)
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
