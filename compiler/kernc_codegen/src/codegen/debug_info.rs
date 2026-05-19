//! DWARF/debug-info emission.
//!
//! Debug-info generation maps modules, functions, locals, parameters, source
//! locations, and type metadata into LLVM DIBuilder nodes while keeping enough
//! state to attach locations during body generation.

use super::CodeGenerator;
use crate::llvm_api::{
    DICompileUnit, DICompositeTypeInput, DIFile, DIFunctionInput, DIMemberTypeInput,
    DIReplaceableCompositeTypeInput, DISubprogram, DIType, DebugInfoBuilder, FunctionValue,
    ModuleFlagBehavior, PointerValue,
};
use kernc_mir::{MirFunction, MirStruct};
use kernc_mono::MonoId;
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_utils::{FileId, Span};
use std::collections::HashMap;
use std::path::Path;

type DebugLayoutField = (String, TypeId, u64, u64, u32);
type DebugStructLayout = (u64, u64, Vec<DebugLayoutField>);

pub(super) struct DebugInfoState<'ctx> {
    builder: DebugInfoBuilder<'ctx>,
    compile_unit: Option<DICompileUnit<'ctx>>,
    primary_file: Option<DIFile<'ctx>>,
    files: HashMap<FileId, DIFile<'ctx>>,
    subprograms: HashMap<MonoId, DISubprogram<'ctx>>,
    types: HashMap<TypeId, DIType<'ctx>>,
    finalized: bool,
}

#[derive(Clone, Copy)]
enum FatPointerMetadataKind {
    Length,
    Pointer,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn ensure_debug_info_state(&mut self) -> Option<&mut DebugInfoState<'ctx>> {
        if !self.debug_info_enabled {
            return None;
        }
        if self.debug_info.is_none() {
            // LLVM expects these module flags to be present before any DI nodes are emitted.
            // We initialize the builder lazily so non-debug builds never pay the setup cost.
            let version = self
                .context
                .i32_type()
                .const_int(self.context.debug_metadata_version() as u64, false);
            self.module.add_basic_value_flag(
                "Debug Info Version",
                ModuleFlagBehavior::Warning,
                version,
            );
            if self.target_uses_coff_sections() {
                let codeview = self.context.i32_type().const_int(1, false);
                self.module
                    .add_basic_value_flag("CodeView", ModuleFlagBehavior::Warning, codeview);
            }
            self.debug_info = Some(DebugInfoState {
                builder: self.module.create_debug_info_builder(),
                compile_unit: None,
                primary_file: None,
                files: HashMap::new(),
                subprograms: HashMap::new(),
                types: HashMap::new(),
                finalized: false,
            });
        }
        self.debug_info.as_mut()
    }

    fn debug_file_parts(path: &Path) -> (String, String) {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown.kn")
            .to_string();
        let directory = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .into_owned();
        (filename, directory)
    }

    fn debug_source_location(&mut self, span: Span) -> Option<(DIFile<'ctx>, u32, u32)> {
        if !self.debug_info_enabled || span == Span::default() {
            return None;
        }
        let location = self.sess.source_manager.lookup_location(span)?;
        let path = self
            .sess
            .source_manager
            .get_file_path(location.file_id)
            .cloned()
            .unwrap_or_default();
        let (filename, directory) = Self::debug_file_parts(&path);
        let state = self.ensure_debug_info_state()?;
        // A single `DIFile` per source file keeps later location emission cheap and ensures
        // identical spans share the same metadata node.
        let file = if let Some(file) = state.files.get(&location.file_id).copied() {
            file
        } else {
            let file = state.builder.create_file(&filename, &directory);
            state.files.insert(location.file_id, file);
            file
        };
        Some((
            file,
            location.line.min(u32::MAX as usize) as u32,
            location.col.min(u32::MAX as usize) as u32,
        ))
    }

    fn debug_pointer_bytes(&self) -> u64 {
        self.sess.target.pointer_size
    }

    fn debug_pointer_bits(&self) -> u64 {
        self.debug_pointer_bytes() * 8
    }

    fn debug_align_to(offset: u64, align: u64) -> u64 {
        if align <= 1 {
            offset
        } else {
            (offset + align - 1) & !(align - 1)
        }
    }

    fn debug_primitive_align_bytes(&self, primitive: PrimitiveType) -> u64 {
        match primitive {
            PrimitiveType::Void | PrimitiveType::Never => 1,
            PrimitiveType::Bool | PrimitiveType::I8 | PrimitiveType::U8 => 1,
            PrimitiveType::I16 | PrimitiveType::U16 => 2,
            PrimitiveType::I32 | PrimitiveType::U32 | PrimitiveType::F32 => 4,
            PrimitiveType::I64 | PrimitiveType::U64 | PrimitiveType::F64 => 8,
            PrimitiveType::ISize | PrimitiveType::USize => self.debug_pointer_bytes(),
            PrimitiveType::I128 | PrimitiveType::U128 => 16,
        }
    }

    fn debug_primitive_size_bytes(&self, primitive: PrimitiveType) -> u64 {
        match primitive {
            PrimitiveType::Void | PrimitiveType::Never => 0,
            PrimitiveType::Bool | PrimitiveType::I8 | PrimitiveType::U8 => 1,
            PrimitiveType::I16 | PrimitiveType::U16 => 2,
            PrimitiveType::I32 | PrimitiveType::U32 | PrimitiveType::F32 => 4,
            PrimitiveType::I64 | PrimitiveType::U64 | PrimitiveType::F64 => 8,
            PrimitiveType::ISize | PrimitiveType::USize => self.debug_pointer_bytes(),
            PrimitiveType::I128 | PrimitiveType::U128 => 16,
        }
    }

    fn debug_has_packed_attr(&self, attrs: &[kernc_ast::MetaItem]) -> bool {
        attrs.iter().any(|attr| {
            matches!(attr, kernc_ast::MetaItem::Marker(id) if self.resolve_symbol(*id) == "packed")
        })
    }

    fn debug_mir_struct_id_for_type(&self, ty: TypeId) -> Option<MonoId> {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Def(def_id, args) | TypeKind::Enum(def_id, args) => {
                self.def_mono_map.get(&(def_id, args)).copied()
            }
            TypeKind::EnumPayload(def_id, args) => self
                .def_mono_map
                .get(&(def_id, args))
                .and_then(|wrapper_id| self.adt_union_map.get(wrapper_id))
                .copied(),
            TypeKind::Range { .. } => self.range_map.get(&norm).copied(),
            TypeKind::AnonymousStruct(..) => self.anon_struct_map.get(&norm).copied(),
            TypeKind::AnonymousUnion(..) => self.anon_union_map.get(&norm).copied(),
            TypeKind::AnonymousEnum(..) => self.anon_enum_map.get(&norm).copied(),
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let enum_ty = self.type_registry.normalize(enum_ty);
                self.anon_enum_map
                    .get(&enum_ty)
                    .and_then(|wrapper_id| self.adt_union_map.get(wrapper_id))
                    .copied()
            }
            _ => None,
        }
    }

    fn debug_mir_struct_for_type(&self, ty: TypeId) -> Option<&MirStruct> {
        let struct_id = self.debug_mir_struct_id_for_type(ty)?;
        self.mir_structs.get(&struct_id)
    }

    fn debug_cached_type(&self, ty: TypeId) -> Option<DIType<'ctx>> {
        self.debug_info
            .as_ref()
            .and_then(|state| state.types.get(&ty).copied())
    }

    fn debug_cache_type(&mut self, ty: TypeId, di_ty: DIType<'ctx>) {
        if let Some(state) = self.ensure_debug_info_state() {
            state.types.insert(ty, di_ty);
        }
    }

    fn debug_type_scope(&mut self) -> Option<(DICompileUnit<'ctx>, DIFile<'ctx>)> {
        let is_optimized = self.debug_info_is_optimized;
        let producer = format!("kernc {}", env!("CARGO_PKG_VERSION"));
        let state = self.ensure_debug_info_state()?;
        // Type metadata can be requested before we have attached any function-level locations.
        // Pin it to the first known source file when possible, and fall back to a synthetic file
        // so recursive type emission always has a compile unit to anchor itself to.
        let file = if let Some(file) = state.primary_file {
            file
        } else if let Some(file) = state.files.values().next().copied() {
            state.primary_file = Some(file);
            file
        } else {
            let file = state.builder.create_file("unknown.kn", ".");
            state.primary_file = Some(file);
            file
        };
        let unit = if let Some(unit) = state.compile_unit {
            unit
        } else {
            let unit = state
                .builder
                .create_compile_unit(file, &producer, is_optimized);
            state.compile_unit = Some(unit);
            unit
        };
        Some((unit, file))
    }

    fn debug_mir_struct_layout(&mut self, mir_struct: &MirStruct) -> DebugStructLayout {
        let packed = self.debug_has_packed_attr(&mir_struct.attributes);
        // MIR already carries the lowered field ordering, but DI needs an explicit byte layout.
        // Rebuild the member offsets here instead of depending on LLVM to reverse-engineer them
        // from the lowered IR types.
        if mir_struct.is_union {
            let align_bytes = if packed {
                1
            } else {
                mir_struct.union_align.max(1) as u64
            };
            let size_bytes = if packed {
                mir_struct.union_size as u64
            } else {
                Self::debug_align_to(mir_struct.union_size as u64, align_bytes)
            };
            let mut members = Vec::with_capacity(mir_struct.fields.len());
            for field in &mir_struct.fields {
                let field_size_bits = self.debug_type_size_bytes(field.ty) * 8;
                let field_align_bits = (if packed {
                    1
                } else {
                    self.debug_type_align_bytes(field.ty).max(1)
                } * 8) as u32;
                members.push((
                    self.resolve_symbol(field.name).to_string(),
                    field.ty,
                    0,
                    field_size_bits,
                    field_align_bits,
                ));
            }
            return (size_bytes.max(1), align_bytes.max(1), members);
        }

        let mut offset_bytes = 0;
        let mut struct_align_bytes = 1;
        let mut members = Vec::with_capacity(mir_struct.fields.len());
        for field in &mir_struct.fields {
            let field_align_bytes = if packed {
                1
            } else {
                self.debug_type_align_bytes(field.ty).max(1)
            };
            let field_size_bytes = self.debug_type_size_bytes(field.ty);
            if !packed {
                struct_align_bytes = struct_align_bytes.max(field_align_bytes);
                offset_bytes = Self::debug_align_to(offset_bytes, field_align_bytes);
            }
            members.push((
                self.resolve_symbol(field.name).to_string(),
                field.ty,
                offset_bytes * 8,
                field_size_bytes * 8,
                (field_align_bytes * 8) as u32,
            ));
            offset_bytes += field_size_bytes;
        }

        let size_bytes = Self::debug_align_to(offset_bytes, struct_align_bytes.max(1));
        (size_bytes, struct_align_bytes.max(1), members)
    }

    fn debug_type_align_bytes(&mut self, ty: TypeId) -> u64 {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => self.debug_primitive_align_bytes(primitive),
            TypeKind::Pointer { .. }
            | TypeKind::VolatilePtr { .. }
            | TypeKind::Function { .. }
            | TypeKind::FnDef(..)
            | TypeKind::Slice { .. }
            | TypeKind::TraitObject(..) => self.debug_pointer_bytes(),
            TypeKind::Simd { elem, lanes } => {
                if elem == TypeId::BOOL {
                    1
                } else {
                    let elem_align = self.debug_type_align_bytes(elem);
                    let elem_size = self.debug_type_size_bytes(elem);
                    elem_align.max(elem_size.saturating_mul(lanes as u64))
                }
            }
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.debug_type_align_bytes(elem).max(1)
            }
            TypeKind::Range { start, end, .. } => {
                let mut align: u64 = 1;
                if let Some(start) = start {
                    align = align.max(self.debug_type_align_bytes(start));
                }
                if let Some(end) = end {
                    align = align.max(self.debug_type_align_bytes(end));
                }
                align.max(1)
            }
            TypeKind::ClosureInterface { .. } => 1,
            TypeKind::AnonymousState { captures, .. } => {
                let mut offset_bytes = 0;
                let mut struct_align_bytes = 1;
                for capture in captures {
                    let capture_align = self.debug_type_align_bytes(capture).max(1);
                    struct_align_bytes = struct_align_bytes.max(capture_align);
                    offset_bytes = Self::debug_align_to(offset_bytes, capture_align);
                    offset_bytes += self.debug_type_size_bytes(capture);
                }
                struct_align_bytes.max(1)
            }
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => self
                .debug_mir_struct_for_type(norm)
                .cloned()
                .map(|mir_struct| self.debug_mir_struct_layout(&mir_struct).1)
                .unwrap_or(1),
            TypeKind::Projection { .. }
            | TypeKind::Alias(..)
            | TypeKind::Param(_)
            | TypeKind::Associated(..)
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            // Keep unresolved or compiler-internal types describable in debuginfo without
            // crashing layout reconstruction. LLVM accepts conservative placeholder sizes.
            | TypeKind::Error => 1,
        }
    }

    fn debug_type_size_bytes(&mut self, ty: TypeId) -> u64 {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => self.debug_primitive_size_bytes(primitive),
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem = self.type_registry.normalize(elem);
                if matches!(
                    self.type_registry.get(elem),
                    TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
                ) {
                    self.debug_pointer_bytes() * 2
                } else {
                    self.debug_pointer_bytes()
                }
            }
            TypeKind::Function { .. } | TypeKind::FnDef(..) => self.debug_pointer_bytes(),
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => self.debug_pointer_bytes() * 2,
            TypeKind::Simd { elem, lanes } => {
                if elem == TypeId::BOOL {
                    (lanes as u64).div_ceil(8)
                } else {
                    self.debug_type_size_bytes(elem)
                        .saturating_mul(lanes as u64)
                }
            }
            TypeKind::Array { elem, len, .. } => self
                .const_generic_usize(len, Span::default())
                .map(|len| self.debug_type_size_bytes(elem).saturating_mul(len))
                .unwrap_or(0),
            TypeKind::ArrayInfer { .. } | TypeKind::ClosureInterface { .. } => 0,
            TypeKind::Range { start, end, .. } => {
                let mut offset_bytes = 0;
                let mut struct_align_bytes: u64 = 1;
                for ty in [start, end].into_iter().flatten() {
                    let align = self.debug_type_align_bytes(ty).max(1);
                    struct_align_bytes = struct_align_bytes.max(align);
                    offset_bytes = Self::debug_align_to(offset_bytes, align);
                    offset_bytes += self.debug_type_size_bytes(ty);
                }
                Self::debug_align_to(offset_bytes, struct_align_bytes.max(1))
            }
            TypeKind::AnonymousState { captures, .. } => {
                let mut offset_bytes = 0;
                let mut struct_align_bytes = 1;
                for capture in captures {
                    let capture_align = self.debug_type_align_bytes(capture).max(1);
                    struct_align_bytes = struct_align_bytes.max(capture_align);
                    offset_bytes = Self::debug_align_to(offset_bytes, capture_align);
                    offset_bytes += self.debug_type_size_bytes(capture);
                }
                Self::debug_align_to(offset_bytes, struct_align_bytes.max(1))
            }
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => self
                .debug_mir_struct_for_type(norm)
                .cloned()
                .map(|mir_struct| self.debug_mir_struct_layout(&mir_struct).0)
                .unwrap_or(0),
            TypeKind::Projection { .. }
            | TypeKind::Alias(..)
            | TypeKind::Param(_)
            | TypeKind::Associated(..)
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            // These forms should normally be normalized away before codegen. Returning zero
            // keeps debug emission lossy but total when we are recovering from earlier errors.
            | TypeKind::Error => 0,
        }
    }

    fn debug_type_name(&self, ty: TypeId) -> String {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => match primitive {
                PrimitiveType::Void => "void".to_string(),
                PrimitiveType::Bool => "bool".to_string(),
                PrimitiveType::I8 => "i8".to_string(),
                PrimitiveType::I16 => "i16".to_string(),
                PrimitiveType::I32 => "i32".to_string(),
                PrimitiveType::I64 => "i64".to_string(),
                PrimitiveType::I128 => "i128".to_string(),
                PrimitiveType::ISize => "isize".to_string(),
                PrimitiveType::U8 => "u8".to_string(),
                PrimitiveType::U16 => "u16".to_string(),
                PrimitiveType::U32 => "u32".to_string(),
                PrimitiveType::U64 => "u64".to_string(),
                PrimitiveType::U128 => "u128".to_string(),
                PrimitiveType::USize => "usize".to_string(),
                PrimitiveType::F32 => "f32".to_string(),
                PrimitiveType::F64 => "f64".to_string(),
                PrimitiveType::Never => "never".to_string(),
            },
            TypeKind::Pointer { is_mut, elem } => {
                format!(
                    "&{}{}",
                    if is_mut { "mut " } else { "" },
                    self.debug_type_name(elem)
                )
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                format!(
                    "^{}{}",
                    if is_mut { "mut " } else { "" },
                    self.debug_type_name(elem)
                )
            }
            TypeKind::Array { elem, len } => format!("[{}]{}", len, self.debug_type_name(elem)),
            TypeKind::Slice { is_mut, elem } => {
                format!(
                    "&{}[{}]",
                    if is_mut { "mut " } else { "" },
                    self.debug_type_name(elem)
                )
            }
            TypeKind::Range {
                start,
                end,
                is_inclusive,
            } => {
                let op = if is_inclusive { "..=" } else { "..." };
                match (start, end) {
                    (Some(start), Some(end)) => {
                        format!(
                            "{}{}{}",
                            self.debug_type_name(start),
                            op,
                            self.debug_type_name(end)
                        )
                    }
                    (Some(start), None) => format!("{}{}", self.debug_type_name(start), op),
                    (None, Some(end)) => format!("{}{}", op, self.debug_type_name(end)),
                    (None, None) => op.to_string(),
                }
            }
            TypeKind::Function { .. } => "&fn".to_string(),
            TypeKind::ClosureInterface { .. } => "Fn".to_string(),
            TypeKind::TraitObject(..) => "trait-object".to_string(),
            TypeKind::Simd { elem, lanes } => format!("{}x{}", self.debug_type_name(elem), lanes),
            TypeKind::AnonymousState { .. } => "<closure-state>".to_string(),
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => self
                .debug_mir_struct_for_type(norm)
                .map(|mir_struct| mir_struct.name.clone())
                .unwrap_or_else(|| "<unnamed>".to_string()),
            TypeKind::Projection { .. }
            | TypeKind::Alias(..)
            | TypeKind::Param(_)
            | TypeKind::Associated(..)
            | TypeKind::FnDef(..)
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::ArrayInfer { .. }
            | TypeKind::Error => "<unnamed>".to_string(),
        }
    }

    fn debug_basic_type_encoding(
        primitive: PrimitiveType,
    ) -> Option<(u64, llvm_sys::debuginfo::LLVMDWARFTypeEncoding)> {
        const DW_ATE_BOOLEAN: u32 = 0x02;
        const DW_ATE_FLOAT: u32 = 0x04;
        const DW_ATE_SIGNED: u32 = 0x05;
        const DW_ATE_UNSIGNED: u32 = 0x07;
        match primitive {
            PrimitiveType::Bool => Some((8, DW_ATE_BOOLEAN)),
            PrimitiveType::I8 => Some((8, DW_ATE_SIGNED)),
            PrimitiveType::I16 => Some((16, DW_ATE_SIGNED)),
            PrimitiveType::I32 => Some((32, DW_ATE_SIGNED)),
            PrimitiveType::I64 => Some((64, DW_ATE_SIGNED)),
            PrimitiveType::I128 => Some((128, DW_ATE_SIGNED)),
            PrimitiveType::ISize => Some((0, DW_ATE_SIGNED)),
            PrimitiveType::U8 => Some((8, DW_ATE_UNSIGNED)),
            PrimitiveType::U16 => Some((16, DW_ATE_UNSIGNED)),
            PrimitiveType::U32 => Some((32, DW_ATE_UNSIGNED)),
            PrimitiveType::U64 => Some((64, DW_ATE_UNSIGNED)),
            PrimitiveType::U128 => Some((128, DW_ATE_UNSIGNED)),
            PrimitiveType::USize => Some((0, DW_ATE_UNSIGNED)),
            PrimitiveType::F32 => Some((32, DW_ATE_FLOAT)),
            PrimitiveType::F64 => Some((64, DW_ATE_FLOAT)),
            PrimitiveType::Void | PrimitiveType::Never => None,
        }
    }

    fn debug_build_named_composite_type(
        &mut self,
        norm: TypeId,
        mir_struct: MirStruct,
    ) -> Option<DIType<'ctx>> {
        const DW_TAG_STRUCTURE_TYPE: u32 = 0x13;
        const DW_TAG_UNION_TYPE: u32 = 0x17;

        let (scope, file) = self.debug_type_scope()?;
        let name = mir_struct.name.clone();
        let unique_id = format!("kern.debug.{name}.{:?}", norm);
        let (size_bytes, align_bytes, members) = self.debug_mir_struct_layout(&mir_struct);
        let placeholder = {
            let state = self.ensure_debug_info_state()?;
            // Recursive ADTs need a replaceable forward declaration first so child members can
            // point back to this type before the final field list is available.
            state
                .builder
                .create_replaceable_composite_type(DIReplaceableCompositeTypeInput {
                    tag: if mir_struct.is_union {
                        DW_TAG_UNION_TYPE
                    } else {
                        DW_TAG_STRUCTURE_TYPE
                    },
                    scope,
                    name: &name,
                    file,
                    size_in_bits: size_bytes * 8,
                    align_in_bits: (align_bytes * 8) as u32,
                    unique_id: &unique_id,
                })
        };
        self.debug_cache_type(norm, placeholder);

        let mut member_types = Vec::with_capacity(members.len());
        for (member_name, member_ty, offset_bits, size_bits, align_bits) in members {
            let field_di_ty = self.debug_type(member_ty)?;
            let member_di = {
                let state = self.ensure_debug_info_state()?;
                state.builder.create_member_type(DIMemberTypeInput {
                    scope,
                    name: &member_name,
                    file,
                    size_in_bits: size_bits,
                    align_in_bits: align_bits,
                    offset_in_bits: offset_bits,
                    ty: field_di_ty,
                })
            };
            member_types.push(member_di);
        }

        let composite_ty = {
            let state = self.ensure_debug_info_state()?;
            if mir_struct.is_union {
                state.builder.create_union_type(DICompositeTypeInput {
                    scope,
                    name: &name,
                    file,
                    size_in_bits: size_bytes * 8,
                    align_in_bits: (align_bytes * 8) as u32,
                    elements: &member_types,
                    unique_id: &unique_id,
                })
            } else {
                state.builder.create_struct_type(DICompositeTypeInput {
                    scope,
                    name: &name,
                    file,
                    size_in_bits: size_bytes * 8,
                    align_in_bits: (align_bytes * 8) as u32,
                    elements: &member_types,
                    unique_id: &unique_id,
                })
            }
        };
        let state = self.ensure_debug_info_state()?;
        state
            .builder
            .replace_all_uses_with(placeholder, composite_ty);
        state.types.insert(norm, composite_ty);
        Some(composite_ty)
    }

    fn debug_build_fat_pointer_type(
        &mut self,
        norm: TypeId,
        data_pointee: DIType<'ctx>,
        meta_name: &str,
        meta_kind: FatPointerMetadataKind,
    ) -> Option<DIType<'ctx>> {
        let (scope, file) = self.debug_type_scope()?;
        let name = self.debug_type_name(norm);
        let pointer_bits = self.debug_pointer_bits();
        // Kern models slices and dynamically-dispatched references as a synthetic two-word
        // struct in debuginfo so debuggers can display both the data pointer and metadata.
        let data_ptr_ty = {
            let state = self.ensure_debug_info_state()?;
            state.builder.create_pointer_type(
                data_pointee,
                pointer_bits,
                pointer_bits as u32,
                "data_ptr",
            )
        };
        let meta_ty = match meta_kind {
            FatPointerMetadataKind::Length => self.debug_type(TypeId::USIZE)?,
            FatPointerMetadataKind::Pointer => {
                let opaque_pointee = self.debug_type(TypeId::VOID)?;
                let state = self.ensure_debug_info_state()?;
                state.builder.create_pointer_type(
                    opaque_pointee,
                    pointer_bits,
                    pointer_bits as u32,
                    meta_name,
                )
            }
        };
        let members = {
            let state = self.ensure_debug_info_state()?;
            vec![
                state.builder.create_member_type(DIMemberTypeInput {
                    scope,
                    name: "data_ptr",
                    file,
                    size_in_bits: pointer_bits,
                    align_in_bits: pointer_bits as u32,
                    offset_in_bits: 0,
                    ty: data_ptr_ty,
                }),
                state.builder.create_member_type(DIMemberTypeInput {
                    scope,
                    name: meta_name,
                    file,
                    size_in_bits: pointer_bits,
                    align_in_bits: pointer_bits as u32,
                    offset_in_bits: pointer_bits,
                    ty: meta_ty,
                }),
            ]
        };
        let composite_ty = {
            let state = self.ensure_debug_info_state()?;
            let unique_id = format!("kern.debug.{name}.{:?}", norm);
            state.builder.create_struct_type(DICompositeTypeInput {
                scope,
                name: &name,
                file,
                size_in_bits: pointer_bits * 2,
                align_in_bits: pointer_bits as u32,
                elements: &members,
                unique_id: &unique_id,
            })
        };
        self.debug_cache_type(norm, composite_ty);
        Some(composite_ty)
    }

    fn debug_build_anonymous_state_type(
        &mut self,
        norm: TypeId,
        captures: Vec<TypeId>,
    ) -> Option<DIType<'ctx>> {
        let (scope, file) = self.debug_type_scope()?;
        let name = self.debug_type_name(norm);
        // Closure environments do not have a named AST item, but debuggers still need a stable
        // synthetic struct so captured locals can be inspected by ordinal.
        let mut offset_bits = 0;
        let mut members = Vec::with_capacity(captures.len());
        for (index, capture_ty) in captures.into_iter().enumerate() {
            let capture_align_bits = (self.debug_type_align_bytes(capture_ty).max(1) * 8) as u32;
            offset_bits = Self::debug_align_to(offset_bits, capture_align_bits as u64);
            let capture_size_bits = self.debug_type_size_bytes(capture_ty) * 8;
            let capture_di_ty = self.debug_type(capture_ty)?;
            let member = {
                let state = self.ensure_debug_info_state()?;
                let member_name = format!("capture{index}");
                state.builder.create_member_type(DIMemberTypeInput {
                    scope,
                    name: &member_name,
                    file,
                    size_in_bits: capture_size_bits,
                    align_in_bits: capture_align_bits,
                    offset_in_bits: offset_bits,
                    ty: capture_di_ty,
                })
            };
            members.push(member);
            offset_bits += capture_size_bits;
        }
        let size_bits = self.debug_type_size_bytes(norm) * 8;
        let align_bits = (self.debug_type_align_bytes(norm) * 8) as u32;
        let composite_ty = {
            let state = self.ensure_debug_info_state()?;
            let unique_id = format!("kern.debug.{name}.{:?}", norm);
            state.builder.create_struct_type(DICompositeTypeInput {
                scope,
                name: &name,
                file,
                size_in_bits: size_bits,
                align_in_bits: align_bits,
                elements: &members,
                unique_id: &unique_id,
            })
        };
        self.debug_cache_type(norm, composite_ty);
        Some(composite_ty)
    }

    fn debug_type(&mut self, ty: TypeId) -> Option<DIType<'ctx>> {
        if !self.debug_info_enabled {
            return None;
        }
        let norm = self.type_registry.normalize(ty);
        if let Some(di_ty) = self.debug_cached_type(norm) {
            return Some(di_ty);
        }

        // Cache entries are keyed by normalized semantic types so every recursive request sees
        // the same metadata node, regardless of which monomorphized site triggered emission.
        let di_ty = match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => {
                let name = self.debug_type_name(norm);
                let type_info = Self::debug_basic_type_encoding(primitive);
                let pointer_bits = self.debug_pointer_bits();
                let state = self.ensure_debug_info_state()?;
                if let Some((mut bits, encoding)) = type_info {
                    if bits == 0 {
                        bits = pointer_bits;
                    }
                    state.builder.create_basic_type(&name, bits, encoding)
                } else {
                    state.builder.create_unspecified_type(&name)
                }
            }
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem = self.type_registry.normalize(elem);
                if matches!(self.type_registry.get(elem), TypeKind::TraitObject(..)) {
                    let data_pointee = self.debug_type(TypeId::VOID)?;
                    return self.debug_build_fat_pointer_type(
                        norm,
                        data_pointee,
                        "vtable",
                        FatPointerMetadataKind::Pointer,
                    );
                }
                if matches!(
                    self.type_registry.get(elem),
                    TypeKind::ClosureInterface { .. }
                ) {
                    let data_pointee = self.debug_type(TypeId::VOID)?;
                    return self.debug_build_fat_pointer_type(
                        norm,
                        data_pointee,
                        "code_ptr",
                        FatPointerMetadataKind::Pointer,
                    );
                }

                let pointee = self.debug_type(elem)?;
                let name = self.debug_type_name(norm);
                let pointer_bits = self.debug_pointer_bits();
                let state = self.ensure_debug_info_state()?;
                state
                    .builder
                    .create_pointer_type(pointee, pointer_bits, pointer_bits as u32, &name)
            }
            TypeKind::Slice { elem, .. } => {
                let data_pointee = self.debug_type(elem)?;
                return self.debug_build_fat_pointer_type(
                    norm,
                    data_pointee,
                    "len",
                    FatPointerMetadataKind::Length,
                );
            }
            TypeKind::TraitObject(..) => {
                let data_pointee = self.debug_type(TypeId::VOID)?;
                return self.debug_build_fat_pointer_type(
                    norm,
                    data_pointee,
                    "vtable",
                    FatPointerMetadataKind::Pointer,
                );
            }
            TypeKind::Array { elem, len, .. } => {
                let elem_di_ty = self.debug_type(elem)?;
                let len = self.const_generic_usize(len, Span::default())?;
                let size_bits = self.debug_type_size_bytes(norm) * 8;
                let align_bits = (self.debug_type_align_bytes(norm) * 8) as u32;
                let state = self.ensure_debug_info_state()?;
                state
                    .builder
                    .create_array_type(elem_di_ty, size_bits, align_bits, len as i64)
            }
            TypeKind::Simd { elem, lanes } => {
                let elem_di_ty = self.debug_type(elem)?;
                let size_bits = self.debug_type_size_bytes(norm) * 8;
                let align_bits = (self.debug_type_align_bytes(norm) * 8) as u32;
                let state = self.ensure_debug_info_state()?;
                state
                    .builder
                    .create_array_type(elem_di_ty, size_bits, align_bits, lanes as i64)
            }
            TypeKind::AnonymousState { captures, .. } => {
                return self.debug_build_anonymous_state_type(norm, captures);
            }
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => {
                if let Some(mir_struct) = self.debug_mir_struct_for_type(norm).cloned() {
                    return self.debug_build_named_composite_type(norm, mir_struct);
                }
                let name = self.debug_type_name(norm);
                let state = self.ensure_debug_info_state()?;
                state.builder.create_unspecified_type(&name)
            }
            _ => {
                let name = self.debug_type_name(norm);
                let state = self.ensure_debug_info_state()?;
                state.builder.create_unspecified_type(&name)
            }
        };
        self.debug_cache_type(norm, di_ty);
        Some(di_ty)
    }

    pub(super) fn declare_debug_local(
        &mut self,
        function: &MirFunction,
        local: &kernc_mir::MirLocal,
        storage: PointerValue<'ctx>,
        entry_block: crate::llvm_api::BasicBlock<'ctx>,
        arg_no: Option<u32>,
    ) {
        let name = self.resolve_symbol(local.name).to_string();
        if name == "<unknown>" {
            return;
        }
        let span = if local.span == Span::default() {
            function.span
        } else {
            local.span
        };
        let Some((file, line, column)) = self.debug_source_location(span) else {
            return;
        };
        let Some(di_ty) = self.debug_type(local.ty) else {
            return;
        };
        let Some(subprogram) = self
            .debug_info
            .as_ref()
            .and_then(|state| state.subprograms.get(&function.id).copied())
        else {
            return;
        };
        let context = self.context;
        let state = self
            .ensure_debug_info_state()
            .expect("debug info state must exist");
        let location = state
            .builder
            .create_debug_location(context, line, column, subprogram);
        let variable = match arg_no {
            Some(arg_no) => state
                .builder
                .create_parameter_variable(subprogram, &name, arg_no, file, line, di_ty),
            None => state
                .builder
                .create_auto_variable(subprogram, &name, file, line, di_ty, 0),
        };
        let expression = state.builder.create_expression();
        let _ = state.builder.insert_declare_at_end(
            storage,
            variable,
            expression,
            location,
            entry_block,
        );
    }

    fn debug_compile_unit(&mut self, file: DIFile<'ctx>) -> Option<DICompileUnit<'ctx>> {
        let is_optimized = self.debug_info_is_optimized;
        let producer = format!("kernc {}", env!("CARGO_PKG_VERSION"));
        let state = self.ensure_debug_info_state()?;
        if state.primary_file.is_none() {
            state.primary_file = Some(file);
        }
        if let Some(unit) = state.compile_unit {
            return Some(unit);
        }
        // Function attachment and type emission can race to request the compile unit; centralize
        // the single-allocation rule here so both paths converge on one `DICompileUnit`.
        let unit = state
            .builder
            .create_compile_unit(file, &producer, is_optimized);
        state.compile_unit = Some(unit);
        Some(unit)
    }

    pub(super) fn attach_debug_info_to_function(
        &mut self,
        function: &MirFunction,
        llvm_func: FunctionValue<'ctx>,
    ) {
        let Some((file, line, _column)) = self.debug_source_location(function.span) else {
            return;
        };
        let Some(compile_unit) = self.debug_compile_unit(file) else {
            return;
        };
        let is_optimized = self.debug_info_is_optimized;
        let is_local_to_unit = matches!(function.linkage, kernc_mir::MirLinkage::Internal);
        let state = self
            .ensure_debug_info_state()
            .expect("debug info state must exist");
        let subroutine_type = state.builder.create_subroutine_type(file);
        let subprogram = state.builder.create_function(DIFunctionInput {
            scope: compile_unit,
            file,
            name: &function.name,
            linkage_name: &function.name,
            line,
            scope_line: line,
            subroutine_type,
            is_local_to_unit,
            is_optimized,
        });
        llvm_func.set_subprogram(subprogram);
        state.subprograms.insert(function.id, subprogram);
    }

    pub(super) fn set_function_debug_location(&mut self, function: &MirFunction) {
        self.set_debug_location_for_span(function, function.span);
    }

    pub(super) fn set_debug_location_for_span(&mut self, function: &MirFunction, span: Span) {
        let Some(subprogram) = self
            .debug_info
            .as_ref()
            .and_then(|state| state.subprograms.get(&function.id).copied())
        else {
            return;
        };
        let Some((_, line, column)) = self.debug_source_location(span) else {
            return;
        };
        let context = self.context;
        let state = self
            .ensure_debug_info_state()
            .expect("debug info state must exist");
        let location = state
            .builder
            .create_debug_location(context, line, column, subprogram);
        self.builder.set_current_debug_location(location);
    }

    pub(super) fn clear_function_debug_location(&mut self) {
        self.builder.clear_current_debug_location();
    }

    pub(super) fn finalize_debug_info(&mut self) {
        if let Some(state) = self.debug_info.as_mut()
            && !state.finalized
        {
            state.builder.finalize();
            state.finalized = true;
        }
    }
}
