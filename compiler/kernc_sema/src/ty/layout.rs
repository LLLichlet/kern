use crate::SemaContext;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_ast as ast;
use kernc_utils::SymbolId;
use std::collections::HashMap;

type StructMapping = (Vec<usize>, Vec<usize>);
type NamedStructMappingKey = (DefId, Vec<TypeId>);

pub struct LayoutEngine<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    align_cache: HashMap<TypeId, u64>,
    size_cache: HashMap<TypeId, u64>,
    named_struct_mapping_cache: HashMap<NamedStructMappingKey, StructMapping>,
}

impl<'a, 'ctx> LayoutEngine<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            align_cache: HashMap::new(),
            size_cache: HashMap::new(),
            named_struct_mapping_cache: HashMap::new(),
        }
    }

    /// Compute the physical layout of a named struct.
    /// Returns `(ast_to_physical, physical_to_ast)`.
    pub fn get_struct_mapping(
        &mut self,
        def_id: DefId,
        generic_args: &[TypeId],
        depth: usize,
    ) -> (Vec<usize>, Vec<usize>) {
        let cache_key = (def_id, generic_args.to_vec());
        if let Some(mapping) = self.named_struct_mapping_cache.get(&cache_key) {
            return mapping.clone();
        }

        let Some(struct_def) = self.lookup_struct_def(def_id, "build a struct layout mapping")
        else {
            return (Vec::new(), Vec::new());
        };
        let map = self.prepare_generic_subst(&struct_def.generics, generic_args);
        let mut field_metas = Vec::new();

        for (ast_idx, field) in struct_def.fields.iter().enumerate() {
            let f_ty = self.resolve_field_type(&field.type_node, &map);
            let f_align = self.compute_type_align_inner(f_ty, depth + 1);
            let f_size = self.compute_type_size_inner(f_ty, depth + 1);
            field_metas.push((ast_idx, f_align, f_size));
        }

        // Optimize layout unless the type is explicitly marked `extern`.
        if !struct_def.is_extern {
            field_metas.sort_by(|a, b| {
                b.1.cmp(&a.1) // 1. Higher alignment first.
                    .then_with(|| b.2.cmp(&a.2)) // 2. Then larger size.
                    .then_with(|| a.0.cmp(&b.0)) // 3. Finally preserve AST order for stability.
            });
        }

        let mut ast_to_physical = vec![0; field_metas.len()];
        let mut physical_to_ast = vec![0; field_metas.len()];

        for (phys_idx, meta) in field_metas.into_iter().enumerate() {
            ast_to_physical[meta.0] = phys_idx;
            physical_to_ast[phys_idx] = meta.0;
        }

        let mapping = (ast_to_physical, physical_to_ast);
        self.named_struct_mapping_cache
            .insert(cache_key, mapping.clone());
        mapping
    }

    /// Compute the physical layout of an anonymous struct.
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

        // Only native anonymous structs participate in layout compaction.
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
        if let Some(&align) = self.align_cache.get(&norm) {
            return align;
        }
        let kind = self.ctx.type_registry.get(norm).clone();

        let align = match kind {
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } | TypeKind::Function { .. } => {
                self.ctx.sess.target.pointer_size
            }
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => self.ctx.sess.target.pointer_size,
            TypeKind::Simd { elem, lanes } => {
                if elem == TypeId::BOOL {
                    1
                } else {
                    let elem_align = self.compute_type_align_inner(elem, depth + 1);
                    let elem_size = self.compute_type_size_inner(elem, depth + 1);
                    elem_align.max(elem_size.saturating_mul(lanes as u64))
                }
            }

            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.compute_type_align_inner(elem, depth + 1)
            }

            TypeKind::Def(def_id, generic_args) | TypeKind::Enum(def_id, generic_args) => {
                self.compute_def_align(def_id, &generic_args, depth)
            }
            TypeKind::AnonymousUnion(_, fields) => {
                let mut max_align = 1;
                for f in fields {
                    let align = self.compute_type_align_inner(f.ty, depth + 1);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let tag_ty = enum_def.backing_ty.unwrap_or(TypeId::U32);
                let mut max_align = self.compute_type_align_inner(tag_ty, depth + 1);
                for variant in &enum_def.variants {
                    if let Some(payload_ty) = variant.payload_ty {
                        let align = self.compute_type_align_inner(payload_ty, depth + 1);
                        if align > max_align {
                            max_align = align;
                        }
                    }
                }
                max_align
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let Some(enum_def) = self.lookup_anonymous_enum(
                    enum_ty,
                    "compute the alignment of an anonymous enum payload",
                ) else {
                    return 1;
                };

                let mut max_align = 1;
                for variant in &enum_def.variants {
                    if let Some(payload_ty) = variant.payload_ty {
                        let align = self.compute_type_align_inner(payload_ty, depth + 1);
                        if align > max_align {
                            max_align = align;
                        }
                    }
                }
                max_align
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
                let Some(def) =
                    self.lookup_enum_def(def_id, "compute the alignment of an enum payload")
                else {
                    return 1;
                };
                let map = self.prepare_generic_subst(&def.generics, &generic_args);
                let mut max_payload_align = 1;
                for v in &def.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, depth + 1);
                        if align > max_payload_align {
                            max_payload_align = align;
                        }
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
        };

        self.align_cache.insert(norm, align);
        align
    }

    pub fn compute_type_size(&mut self, ty: TypeId) -> u64 {
        self.compute_type_size_inner(ty, 0)
    }

    fn compute_type_size_inner(&mut self, ty: TypeId, depth: usize) -> u64 {
        if depth > 100 {
            return 0;
        }

        let norm = self.ctx.type_registry.normalize(ty);
        if let Some(&size) = self.size_cache.get(&norm) {
            return size;
        }
        let kind = self.ctx.type_registry.get(norm).clone();

        let size = match kind {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(elem);
                if matches!(
                    self.ctx.type_registry.get(elem_norm),
                    TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
                ) {
                    self.ctx.sess.target.pointer_size * 2
                } else {
                    self.ctx.sess.target.pointer_size
                }
            }
            TypeKind::Function { .. } => self.ctx.sess.target.pointer_size,
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => {
                self.ctx.sess.target.pointer_size * 2
            }
            TypeKind::Simd { elem, lanes } => {
                if elem == TypeId::BOOL {
                    (lanes as u64).div_ceil(8)
                } else {
                    self.compute_type_size_inner(elem, depth + 1) * lanes as u64
                }
            }

            // Fixed-size arrays have known size; `ArrayInfer` still counts as unknown here.
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
                    // Align the running offset to the field's requirement.
                    offset = Self::align_to(offset, f_align);
                    offset += f_size;
                }
                // Apply final tail padding up to the maximum alignment.
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
                    if f_align > max_align {
                        max_align = f_align;
                    }
                    offset = Self::align_to(offset, f_align);
                    offset += f_size;
                }
                Self::align_to(offset, max_align)
            }
            TypeKind::AnonymousUnion(_, fields) => {
                let mut max_size = 0;
                for field in fields {
                    let size = self.compute_type_size_inner(field.ty, depth + 1);
                    if size > max_size {
                        max_size = size;
                    }
                }
                max_size
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let tag_ty = enum_def.backing_ty.unwrap_or(TypeId::U32);
                let tag_size = self.compute_type_size_inner(tag_ty, depth + 1);
                let tag_align = self.compute_type_align_inner(tag_ty, depth + 1);
                let payload_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnumPayload(norm));
                let payload_size = self.compute_type_size_inner(payload_ty, depth + 1);
                let payload_align = self.compute_type_align_inner(payload_ty, depth + 1);
                let max_align = tag_align.max(payload_align);
                let payload_offset = Self::align_to(tag_size, payload_align);
                Self::align_to(payload_offset + payload_size, max_align)
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let Some(enum_def) = self.lookup_anonymous_enum(
                    enum_ty,
                    "compute the size of an anonymous enum payload",
                ) else {
                    return 0;
                };

                let mut max_payload_size = 0;
                for variant in &enum_def.variants {
                    if let Some(payload_ty) = variant.payload_ty {
                        let size = self.compute_type_size_inner(payload_ty, depth + 1);
                        if size > max_payload_size {
                            max_payload_size = size;
                        }
                    }
                }
                max_payload_size
            }
            TypeKind::Error | TypeKind::Primitive(PrimitiveType::Never) => 0,
            TypeKind::Primitive(p) => self.primitive_size(p),

            TypeKind::EnumPayload(def_id, generic_args) => {
                let Some(def) = self.lookup_enum_def(def_id, "compute the size of an enum payload")
                else {
                    return 0;
                };
                let map = self.prepare_generic_subst(&def.generics, &generic_args);
                let mut max_payload_size = 0;
                for v in &def.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let size = self.compute_type_size_inner(p_ty, depth + 1);
                        if size > max_payload_size {
                            max_payload_size = size;
                        }
                    }
                }
                max_payload_size
            }

            TypeKind::TypeVar(_) => {
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    "Kern ICE (Layout): Cannot compute the size of an unresolved TypeVar.",
                );
                0
            }
            _ => {
                self.ctx.emit_ice(kernc_utils::Span::default(), format!("Kern ICE (Layout): Cannot compute the size of an invalid or incomplete type: {:?}", kind));
                0
            }
        };

        self.size_cache.insert(norm, size);
        size
    }

    fn align_to(offset: u64, align: u64) -> u64 {
        (offset + align - 1) & !(align - 1)
    }

    fn lookup_struct_def(&mut self, def_id: DefId, context: &str) -> Option<crate::def::StructDef> {
        match self.ctx.defs.get(def_id.0 as usize).cloned() {
            Some(Def::Struct(def)) => Some(def),
            Some(other) => {
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Layout): Expected struct definition while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
            None => {
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Layout): Missing DefId {} while trying to {}.",
                        def_id.0, context
                    ),
                );
                None
            }
        }
    }

    fn lookup_enum_def(&mut self, def_id: DefId, context: &str) -> Option<crate::def::EnumDef> {
        match self.ctx.defs.get(def_id.0 as usize).cloned() {
            Some(Def::Enum(def)) => Some(def),
            Some(other) => {
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Layout): Expected enum definition while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
            None => {
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Layout): Missing DefId {} while trying to {}.",
                        def_id.0, context
                    ),
                );
                None
            }
        }
    }

    fn lookup_anonymous_enum(
        &mut self,
        enum_ty: TypeId,
        context: &str,
    ) -> Option<crate::ty::AnonymousEnum> {
        let enum_ty = self.ctx.type_registry.normalize(enum_ty);
        match self.ctx.type_registry.get(enum_ty).clone() {
            TypeKind::AnonymousEnum(enum_def) => Some(enum_def),
            other => {
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Layout): Expected anonymous enum while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
        }
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
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let field = &s.fields[ast_idx];
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, depth + 1);

                    if f_align > max_align {
                        max_align = f_align;
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
                let (_, physical_to_ast) = self.get_struct_mapping(def_id, generic_args, depth);
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut offset = 0;
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let field = &s.fields[ast_idx];
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
                // C-ABI tagged-union layout: `struct { TagType tag; union { ... } payload; }`.
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

                // Track the maximum payload-union size and alignment.
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

                // 1. Compute the enum's overall alignment.
                let enum_align = tag_align.max(max_payload_align);

                // Pure enums with no payload occupy only the aligned tag.
                if max_payload_size == 0 {
                    return Self::align_to(tag_size, enum_align);
                }

                // 2. Compute the payload start offset under union alignment rules.
                let payload_offset = Self::align_to(tag_size, max_payload_align);

                // 3. Compute the final size and apply tail padding.
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
