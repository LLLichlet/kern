use crate::SemaContext;

use super::{PrimitiveType, TypeId, TypeKind};

pub struct TypeFormatter<'a, 'ctx> {
    pub ctx: &'a SemaContext<'ctx>,
}

impl<'a, 'ctx> TypeFormatter<'a, 'ctx> {
    pub fn format(&self, ty: TypeId) -> String {
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

            TypeKind::Error => "{error}".to_string(),
        }
    }
}
