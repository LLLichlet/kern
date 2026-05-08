use crate::SemaContext;
use crate::def::{Def, DefId};
use crate::ty::{ConstGeneric, GenericArg, PrimitiveType, Substituter, TypeId, TypeKind};
use kernc_ast as ast;
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

type StructMapping = (Vec<usize>, Vec<usize>);
type NamedStructMappingKey = (DefId, Vec<GenericArg>);

#[derive(Clone, Copy)]
struct ActiveLayoutFrame {
    ty: TypeId,
    span: Span,
}

pub struct LayoutEngine<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    align_cache: HashMap<TypeId, u64>,
    size_cache: HashMap<TypeId, u64>,
    named_struct_mapping_cache: HashMap<NamedStructMappingKey, StructMapping>,
    active_layout_stack: Vec<ActiveLayoutFrame>,
}

impl<'a, 'ctx> LayoutEngine<'a, 'ctx> {
    fn emit_invalid_layout_request(
        &mut self,
        span: Span,
        action: &str,
        subject: impl Into<String>,
    ) {
        let span = if span == Span::default() {
            Span::default()
        } else {
            span
        };
        self.ctx
            .struct_error(span, format!("cannot {} {}", action, subject.into()))
            .emit();
    }

    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            align_cache: HashMap::new(),
            size_cache: HashMap::new(),
            named_struct_mapping_cache: HashMap::new(),
            active_layout_stack: Vec::new(),
        }
    }

    /// Compute the physical layout of a named struct.
    /// Returns `(ast_to_physical, physical_to_ast)`.
    pub fn get_struct_mapping(
        &mut self,
        def_id: DefId,
        generic_args: &[GenericArg],
        _depth: usize,
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
            let f_align = self.compute_type_align_inner(f_ty, field.type_node.span);
            let f_size = self.compute_type_size_inner(f_ty, field.type_node.span);
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
        _depth: usize,
    ) -> (Vec<usize>, Vec<usize>) {
        let mut field_metas = Vec::new();
        for (ast_idx, f) in fields.iter().enumerate() {
            let f_align = self.compute_type_align_inner(f.ty, Span::default());
            let f_size = self.compute_type_size_inner(f.ty, Span::default());
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
        self.compute_type_align_inner(ty, self.layout_request_span(ty))
    }

    fn compute_type_align_inner(&mut self, ty: TypeId, request_span: Span) -> u64 {
        let norm = self.ctx.normalize_concrete_type(ty);
        if let Some(&align) = self.align_cache.get(&norm) {
            return align;
        }

        if let Some(ancestor_index) = self
            .active_layout_stack
            .iter()
            .position(|frame| frame.ty == norm)
        {
            self.emit_recursive_layout_diagnostic(ancestor_index, norm, request_span);
            return 1;
        }

        self.active_layout_stack.push(ActiveLayoutFrame {
            ty: norm,
            span: request_span,
        });
        let align = self.compute_type_align_body(norm, request_span);
        self.active_layout_stack.pop();
        self.align_cache.insert(norm, align);
        align
    }

    fn compute_type_align_body(&mut self, norm: TypeId, request_span: Span) -> u64 {
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } | TypeKind::Function { .. } => {
                self.ctx.sess.target.pointer_size
            }
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => self.ctx.sess.target.pointer_size,
            TypeKind::Simd { elem, lanes } => {
                if elem == TypeId::BOOL {
                    1
                } else {
                    let elem_align = self.compute_type_align_inner(elem, request_span);
                    let elem_size = self.compute_type_size_inner(elem, request_span);
                    elem_align.max(elem_size.saturating_mul(lanes as u64))
                }
            }

            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.compute_type_align_inner(elem, request_span)
            }

            TypeKind::Def(def_id, generic_args) | TypeKind::Enum(def_id, generic_args) => {
                self.compute_def_align(def_id, &generic_args)
            }
            TypeKind::AnonymousUnion(_, fields) => {
                let mut max_align = 1;
                for f in fields {
                    let align = self.compute_type_align_inner(f.ty, request_span);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let tag_ty = enum_def.backing_ty.unwrap_or(TypeId::U32);
                let mut max_align = self.compute_type_align_inner(tag_ty, request_span);
                for variant in &enum_def.variants {
                    if let Some(payload_ty) = variant.payload_ty {
                        let align = self.compute_type_align_inner(payload_ty, variant.name_span);
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
                        let align = self.compute_type_align_inner(payload_ty, variant.name_span);
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
                    let align = self.compute_type_align_inner(cap_ty, request_span);
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
                    let align = self.compute_type_align_inner(f.ty, request_span);
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
                        let align = self.compute_type_align_inner(p_ty, payload.span);
                        if align > max_payload_align {
                            max_payload_align = align;
                        }
                    }
                }
                max_payload_align
            }

            TypeKind::TypeVar(_) => {
                self.emit_invalid_layout_request(
                    request_span,
                    "compute the alignment of",
                    "an unresolved inferred type",
                );
                1
            }
            _ => {
                self.emit_invalid_layout_request(
                    request_span,
                    "compute the alignment of",
                    format!(
                        "an invalid or incomplete type `{}`",
                        self.type_display(norm)
                    ),
                );
                1
            }
        }
    }

    pub fn compute_type_size(&mut self, ty: TypeId) -> u64 {
        self.compute_type_size_inner(ty, self.layout_request_span(ty))
    }

    pub fn compute_type_size_at(&mut self, ty: TypeId, span: Span) -> u64 {
        self.compute_type_size_inner(ty, span)
    }

    fn compute_type_size_inner(&mut self, ty: TypeId, request_span: Span) -> u64 {
        let norm = self.ctx.normalize_concrete_type(ty);
        if let Some(&size) = self.size_cache.get(&norm) {
            return size;
        }

        if let Some(ancestor_index) = self
            .active_layout_stack
            .iter()
            .position(|frame| frame.ty == norm)
        {
            self.emit_recursive_layout_diagnostic(ancestor_index, norm, request_span);
            return 0;
        }

        self.active_layout_stack.push(ActiveLayoutFrame {
            ty: norm,
            span: request_span,
        });
        let size = self.compute_type_size_body(norm, request_span);
        self.active_layout_stack.pop();
        self.size_cache.insert(norm, size);
        size
    }

    fn compute_type_size_body(&mut self, norm: TypeId, request_span: Span) -> u64 {
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
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
                    self.compute_type_size_inner(elem, request_span) * lanes as u64
                }
            }

            // Fixed-size arrays have known size; `ArrayInfer` still counts as unknown here.
            TypeKind::Array { elem, len, .. } => {
                let Some(len) = self.resolve_array_len(len, request_span) else {
                    return 0;
                };
                self.compute_type_size_inner(elem, request_span) * len
            }
            TypeKind::ArrayInfer { .. } => {
                self.emit_invalid_layout_request(
                    request_span,
                    "compute the size of",
                    "an array with inferred length `[_]T`",
                );
                0
            }

            TypeKind::Def(def_id, generic_args) | TypeKind::Enum(def_id, generic_args) => {
                self.compute_def_size(def_id, &generic_args)
            }
            TypeKind::AnonymousState { captures, .. } => {
                let mut offset = 0;
                let mut max_align = 1;

                for cap_ty in captures {
                    let f_align = self.compute_type_align_inner(cap_ty, request_span);
                    let f_size = self.compute_type_size_inner(cap_ty, request_span);

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
                let (_, physical_to_ast) = self.get_anon_struct_mapping(is_extern, &fields, 0);
                let mut offset = 0;
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let f = &fields[ast_idx];
                    let f_align = self.compute_type_align_inner(f.ty, request_span);
                    let f_size = self.compute_type_size_inner(f.ty, request_span);
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
                    let size = self.compute_type_size_inner(field.ty, request_span);
                    if size > max_size {
                        max_size = size;
                    }
                }
                max_size
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let tag_ty = enum_def.backing_ty.unwrap_or(TypeId::U32);
                let tag_size = self.compute_type_size_inner(tag_ty, request_span);
                let tag_align = self.compute_type_align_inner(tag_ty, request_span);
                let payload_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnumPayload(norm));
                let payload_size = self.compute_type_size_inner(payload_ty, request_span);
                let payload_align = self.compute_type_align_inner(payload_ty, request_span);
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
                        let size = self.compute_type_size_inner(payload_ty, variant.name_span);
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
                        let size = self.compute_type_size_inner(p_ty, payload.span);
                        if size > max_payload_size {
                            max_payload_size = size;
                        }
                    }
                }
                max_payload_size
            }

            TypeKind::TypeVar(_) => {
                self.emit_invalid_layout_request(
                    request_span,
                    "compute the size of",
                    "an unresolved inferred type",
                );
                0
            }
            _ => {
                self.emit_invalid_layout_request(
                    request_span,
                    "compute the size of",
                    format!(
                        "an invalid or incomplete type `{}`",
                        self.type_display(norm)
                    ),
                );
                0
            }
        }
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

    fn compute_def_align(&mut self, def_id: DefId, generic_args: &[GenericArg]) -> u64 {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        match def {
            Def::Struct(s) => {
                let (_, physical_to_ast) = self.get_struct_mapping(def_id, generic_args, 0);
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let field = &s.fields[ast_idx];
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, field.type_node.span);

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
                    let align = self.compute_type_align_inner(f_ty, field.type_node.span);
                    if align > max_align {
                        max_align = align;
                    }
                }
                max_align
            }
            Def::Enum(a) => {
                let tag_ty = a.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx.node_type(bt.id).unwrap_or(TypeId::U32)
                });
                let mut max_align = self.compute_type_align_inner(
                    tag_ty,
                    a.backing_type.as_ref().map_or(a.span, |bt| bt.span),
                );

                let map = self.prepare_generic_subst(&a.generics, generic_args);
                for v in &a.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, payload.span);
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

    fn compute_def_size(&mut self, def_id: DefId, generic_args: &[GenericArg]) -> u64 {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        match def {
            Def::Struct(s) => {
                let (_, physical_to_ast) = self.get_struct_mapping(def_id, generic_args, 0);
                let map = self.prepare_generic_subst(&s.generics, generic_args);
                let mut offset = 0;
                let mut max_align = 1;

                for &ast_idx in &physical_to_ast {
                    let field = &s.fields[ast_idx];
                    let f_ty = self.resolve_field_type(&field.type_node, &map);
                    let f_align = self.compute_type_align_inner(f_ty, field.type_node.span);
                    let f_size = self.compute_type_size_inner(f_ty, field.type_node.span);

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
                    let f_align = self.compute_type_align_inner(f_ty, field.type_node.span);
                    let f_size = self.compute_type_size_inner(f_ty, field.type_node.span);

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
                    self.ctx.node_type(bt.id).unwrap_or(TypeId::U32)
                });

                let tag_span = a.backing_type.as_ref().map_or(a.span, |bt| bt.span);
                let tag_align = self.compute_type_align_inner(tag_ty, tag_span);
                let tag_size = self.compute_type_size_inner(tag_ty, tag_span);

                let map = self.prepare_generic_subst(&a.generics, generic_args);

                // Track the maximum payload-union size and alignment.
                let mut max_payload_size = 0;
                let mut max_payload_align = 1;

                for v in &a.variants {
                    if let Some(payload) = &v.payload_type {
                        let p_ty = self.resolve_field_type(payload, &map);
                        let align = self.compute_type_align_inner(p_ty, payload.span);
                        let size = self.compute_type_size_inner(p_ty, payload.span);

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
        args: &[GenericArg],
    ) -> HashMap<SymbolId, GenericArg> {
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
        map: &HashMap<SymbolId, GenericArg>,
    ) -> TypeId {
        let mut f_ty = self.ctx.node_type_or_error(type_node.id);
        if !map.is_empty() {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, map);
            f_ty = subst.substitute(f_ty);
        }
        f_ty
    }

    fn resolve_array_len(&mut self, len: ConstGeneric, request_span: Span) -> Option<u64> {
        match len {
            ConstGeneric::Value(value) => {
                let len = u64::try_from(value.as_int()?).ok()?;
                if len > u32::MAX as u64 {
                    self.ctx
                        .struct_error(
                            request_span,
                            format!(
                                "array length {} exceeds the current compiler limit of {} elements",
                                len,
                                u32::MAX
                            ),
                        )
                        .with_hint(
                            "LLVM array types are emitted with a 32-bit element count; split the object or allocate dynamically instead",
                        )
                        .emit();
                    return None;
                }
                Some(len)
            }
            ConstGeneric::Param(symbol, _) => {
                self.emit_invalid_layout_request(
                    request_span,
                    "compute the layout of an array whose length depends on",
                    format!(
                        "the unresolved const generic `{}`",
                        self.ctx.resolve(symbol)
                    ),
                );
                None
            }
            ConstGeneric::Expr(expr_id) => {
                self.emit_invalid_layout_request(
                    request_span,
                    "compute the layout of an array whose length depends on",
                    format!(
                        "the unresolved const expression `{}`",
                        ConstGeneric::Expr(expr_id)
                    ),
                );
                None
            }
            ConstGeneric::Error => None,
        }
    }

    fn layout_request_span(&self, ty: TypeId) -> Span {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Def(def_id, _) | TypeKind::Enum(def_id, _) => self.def_span(*def_id),
            TypeKind::Projection { assoc_def_id, .. } => self.def_span(*assoc_def_id),
            _ => Span::default(),
        }
    }

    fn def_span(&self, def_id: DefId) -> Span {
        match self.ctx.defs.get(def_id.0 as usize) {
            Some(Def::Struct(def)) => def.span,
            Some(Def::Union(def)) => def.span,
            Some(Def::Enum(def)) => def.span,
            Some(Def::Trait(def)) => def.span,
            Some(Def::AssociatedType(def)) => def.span,
            Some(Def::Impl(def)) => def.span,
            Some(Def::TypeAlias(def)) => def.span,
            Some(Def::Function(def)) => def.span,
            Some(Def::Global(def)) => def.span,
            Some(Def::Module(_)) | None => Span::default(),
        }
    }

    fn type_display(&self, ty: TypeId) -> String {
        self.ctx.ty_to_string(ty)
    }

    fn emit_recursive_layout_diagnostic(
        &mut self,
        ancestor_index: usize,
        ty: TypeId,
        request_span: Span,
    ) {
        let cycle_frames = &self.active_layout_stack[ancestor_index..];
        let mut cycle_types = cycle_frames
            .iter()
            .map(|frame| frame.ty)
            .collect::<Vec<_>>();
        cycle_types.push(ty);

        if cycle_types.iter().any(|cycle_ty| {
            self.ctx
                .analysis
                .recursive_reports
                .reported_recursive_layout_types
                .contains(cycle_ty)
        }) {
            return;
        }

        for cycle_ty in &cycle_types {
            self.ctx
                .analysis
                .recursive_reports
                .reported_recursive_layout_types
                .insert(*cycle_ty);
        }

        let type_name = self.type_display(ty);
        let mut chain = cycle_frames
            .iter()
            .map(|frame| self.type_display(frame.ty))
            .collect::<Vec<_>>();
        chain.push(type_name.clone());

        let primary_span = if request_span != Span::default() {
            request_span
        } else {
            cycle_frames
                .last()
                .map(|frame| frame.span)
                .filter(|span| *span != Span::default())
                .unwrap_or_else(|| self.layout_request_span(ty))
        };

        let mut labels = Vec::new();
        let mut labeled_types = Vec::new();
        for cycle_ty in cycle_types {
            if labeled_types.contains(&cycle_ty) {
                continue;
            }
            labeled_types.push(cycle_ty);

            let label_span = match self.ctx.type_registry.get(cycle_ty) {
                TypeKind::Def(def_id, _) | TypeKind::Enum(def_id, _) => self.def_span(*def_id),
                _ => Span::default(),
            };
            if label_span == Span::default() {
                continue;
            }

            labels.push((
                label_span,
                format!("type `{}` is declared here", self.type_display(cycle_ty)),
            ));
        }

        let mut diag = self
            .ctx
            .struct_error(
                primary_span,
                format!(
                    "type `{}` recursively contains itself by value and therefore has infinite size",
                    type_name
                ),
            )
            .with_hint(format!("recursive layout chain: {}", chain.join(" -> ")))
            .with_hint("break the cycle with an explicit pointer such as `&T` or `&mut T`");

        for (label_span, label) in labels {
            diag = diag.with_span_label(label_span, label);
        }

        diag.emit();
    }
}

#[cfg(test)]
mod tests {
    use super::LayoutEngine;
    use crate::SemaContext;
    use crate::ty::{
        ConstExprBinaryOp, ConstExprKind, ConstGeneric, ConstGenericValue, ConstGenericValueKind,
        TypeId, TypeKind,
    };
    use kernc_utils::{DiagnosticLevel, Session, Span};

    #[test]
    fn unresolved_type_var_layout_is_reported_as_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let ty = ctx.type_registry.intern(TypeKind::TypeVar(0));

        let size = LayoutEngine::new(&mut ctx).compute_type_size_at(ty, Span::default());

        assert_eq!(size, 0);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "cannot compute the size of an unresolved inferred type"
        );
    }

    #[test]
    fn unresolved_const_expr_array_len_is_reported_as_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let symbol = ctx.intern("N");
        let expr = ctx.type_registry.intern_const_expr(ConstExprKind::Binary {
            op: ConstExprBinaryOp::Add,
            lhs: ConstGeneric::Param(symbol, TypeId::USIZE),
            rhs: ConstGeneric::Value(ConstGenericValue {
                ty: TypeId::USIZE,
                kind: ConstGenericValueKind::Int(1),
            }),
            ty: TypeId::USIZE,
        });
        let ty = ctx.type_registry.intern(TypeKind::Array {
            elem: TypeId::U8,
            len: ConstGeneric::Expr(expr),
        });

        let size = LayoutEngine::new(&mut ctx).compute_type_size_at(ty, Span::default());

        assert_eq!(size, 0);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert!(ctx.sess.diagnostics[0]
            .message
            .starts_with("cannot compute the layout of an array whose length depends on the unresolved const expression `<const-expr:"));
    }
}
