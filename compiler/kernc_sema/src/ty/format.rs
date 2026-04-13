use crate::SemaContext;

use super::{BuiltinAnonymousEnumKind, PrimitiveType, TypeId, TypeKind};

pub(crate) struct TypeFormatter<'a, 'ctx> {
    pub(crate) ctx: &'a SemaContext<'ctx>,
}

impl<'a, 'ctx> TypeFormatter<'a, 'ctx> {
    pub(crate) fn format(&self, ty: TypeId) -> String {
        let kind = self.ctx.type_registry.get(ty);
        match kind {
            TypeKind::Primitive(p) => match p {
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
                PrimitiveType::Str => "str".to_string(),
                PrimitiveType::Never => "!".to_string(),
            },
            TypeKind::Simd { elem, lanes } => format!("{}x{}", self.format(*elem), lanes),
            TypeKind::Pointer { is_mut, elem } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("*{}{}", m, self.format(*elem))
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("^{}{}", m, self.format(*elem))
            }
            TypeKind::Slice { is_mut, elem } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("[]{}{}", m, self.format(*elem))
            }
            TypeKind::Array { is_mut, elem, len } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("[{}]{}{}", len, m, self.format(*elem))
            }
            TypeKind::ArrayInfer { is_mut, elem } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("[_]{}{}", m, self.format(*elem))
            }
            TypeKind::TypeVar(vid) => format!("?T{}", vid),

            TypeKind::Def(def_id, generics)
            | TypeKind::TraitObject(def_id, generics)
            | TypeKind::Enum(def_id, generics) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<anonymous>");
                if generics.is_empty() {
                    name.to_string()
                } else {
                    let gen_strs: Vec<String> = generics.iter().map(|g| self.format(*g)).collect();
                    format!("{}[{}]", name, gen_strs.join(", "))
                }
            }

            TypeKind::EnumPayload(def_id, generics) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<anonymous>");
                if generics.is_empty() {
                    format!("{}::Payload", name)
                } else {
                    let gen_strs: Vec<String> = generics.iter().map(|g| self.format(*g)).collect();
                    format!("{}::Payload[{}]", name, gen_strs.join(", "))
                }
            }

            TypeKind::Alias(sym, _) => self.ctx.resolve(*sym).to_string(),
            TypeKind::Param(sym) => self.ctx.resolve(*sym).to_string(),
            TypeKind::Associated(def_id, generics) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<associated>");
                if generics.is_empty() {
                    name.to_string()
                } else {
                    let gen_strs: Vec<String> = generics.iter().map(|g| self.format(*g)).collect();
                    format!("{}[{}]", name, gen_strs.join(", "))
                }
            }

            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let mut param_strs: Vec<String> = params.iter().map(|p| self.format(*p)).collect();
                if *is_variadic {
                    param_strs.push("...".to_string());
                }
                format!("fn({}) {}", param_strs.join(", "), self.format(*ret))
            }

            TypeKind::FnDef(def_id, generics) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<anonymous fn>");
                if generics.is_empty() {
                    format!("fn item `{}`", name)
                } else {
                    let gen_strs: Vec<String> = generics.iter().map(|g| self.format(*g)).collect();
                    format!("fn item `{}[{}]`", name, gen_strs.join(", "))
                }
            }

            TypeKind::Module(def_id) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<anonymous>");
                format!("module `{}`", name)
            }

            TypeKind::ClosureInterface { params, ret } => {
                let param_strs: Vec<String> = params.iter().map(|p| self.format(*p)).collect();
                format!("Fn({}) {}", param_strs.join(", "), self.format(*ret))
            }

            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                let cap_strs: Vec<String> = captures.iter().map(|c| self.format(*c)).collect();
                let param_strs: Vec<String> = params.iter().map(|p| self.format(*p)).collect();
                format!(
                    "[closure_state captures:({}) params:({}) -> {}]",
                    cap_strs.join(", "),
                    param_strs.join(", "),
                    self.format(*ret)
                )
            }

            TypeKind::AnonymousStruct(is_extern, fields) => {
                let prefix = if *is_extern {
                    "extern struct"
                } else {
                    "struct"
                };
                if fields.is_empty() {
                    format!("{} {{}}", prefix)
                } else {
                    let field_strs: Vec<String> = fields
                        .iter()
                        .map(|f| format!("{}: {}", self.ctx.resolve(f.name), self.format(f.ty)))
                        .collect();
                    format!("{} {{ {} }}", prefix, field_strs.join(", "))
                }
            }

            TypeKind::AnonymousUnion(is_extern, fields) => {
                let prefix = if *is_extern { "extern union" } else { "union" };
                if fields.is_empty() {
                    format!("{} {{}}", prefix)
                } else {
                    let field_strs: Vec<String> = fields
                        .iter()
                        .map(|f| format!("{}: {}", self.ctx.resolve(f.name), self.format(f.ty)))
                        .collect();
                    format!("{} {{ {} }}", prefix, field_strs.join(", "))
                }
            }

            TypeKind::AnonymousEnum(enum_def) => {
                match enum_def.builtin {
                    Some(BuiltinAnonymousEnumKind::Optional) => {
                        if let Some(inner) = enum_def.builtin_optional_payload() {
                            return format!("?{}", self.format(inner));
                        }
                    }
                    Some(BuiltinAnonymousEnumKind::Result) => {
                        if let Some((ok, err)) = enum_def.builtin_result_types() {
                            return format!("{}!{}", self.format(ok), self.format(err));
                        }
                    }
                    None => {}
                }

                let mut parts = Vec::new();
                for variant in &enum_def.variants {
                    let mut part = self.ctx.resolve(variant.name).to_string();
                    if let Some(payload_ty) = variant.payload_ty {
                        part.push_str(": ");
                        part.push_str(&self.format(payload_ty));
                    }
                    if let Some(value) = variant.explicit_value {
                        part.push_str(" = ");
                        part.push_str(&value.to_string());
                    }
                    parts.push(part);
                }

                if let Some(backing_ty) = enum_def.backing_ty {
                    format!(
                        "enum: {} {{ {} }}",
                        self.format(backing_ty),
                        parts.join(", ")
                    )
                } else {
                    format!("enum {{ {} }}", parts.join(", "))
                }
            }

            TypeKind::AnonymousEnumPayload(enum_ty) => {
                format!("[anon-enum-payload {}]", self.format(*enum_ty))
            }

            TypeKind::Error => "{error}".to_string(),
        }
    }
}
