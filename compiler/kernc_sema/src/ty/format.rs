use crate::SemaContext;
use crate::def::Def;

use super::{
    BuiltinAnonymousEnumKind, ConstExprBinaryOp, ConstExprUnaryOp, ConstGeneric, GenericArg,
    PrimitiveType, TypeId, TypeKind,
};
use kernc_ast::{BinaryOperator, Expr, ExprKind, UnaryOperator};

pub(crate) struct TypeFormatter<'a, 'ctx> {
    pub(crate) ctx: &'a SemaContext<'ctx>,
}

impl<'a, 'ctx> TypeFormatter<'a, 'ctx> {
    fn format_const_generic_value(&self, value: super::ConstGenericValue) -> String {
        match value.kind {
            super::ConstGenericValueKind::Bool(value) => value.to_string(),
            super::ConstGenericValueKind::Int(tag) => self
                .format_enum_const_generic_value(value.ty, tag)
                .unwrap_or_else(|| tag.to_string()),
        }
    }

    fn format_enum_const_generic_value(&self, ty: TypeId, tag: i128) -> Option<String> {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Enum(def_id, generics) => {
                let variant = self.named_enum_variant_name_for_tag(*def_id, tag)?;
                let enum_name = self.ctx.defs[def_id.0 as usize]
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<anonymous>");
                let mut out = enum_name.to_string();
                if !generics.is_empty() {
                    let args = generics
                        .iter()
                        .map(|arg| self.format_generic_arg(*arg))
                        .collect::<Vec<_>>();
                    out.push('[');
                    out.push_str(&args.join(", "));
                    out.push(']');
                }
                out.push('.');
                out.push_str(self.ctx.resolve(variant));
                Some(out)
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let variant = self.anonymous_enum_variant_name_for_tag(enum_def, tag)?;
                Some(format!(".{}", self.ctx.resolve(variant)))
            }
            _ => None,
        }
    }

    fn named_enum_variant_name_for_tag(
        &self,
        def_id: crate::def::DefId,
        tag: i128,
    ) -> Option<kernc_utils::SymbolId> {
        let Def::Enum(enum_def) = self.ctx.defs.get(def_id.0 as usize)? else {
            return None;
        };

        let mut current_tag = 0i128;
        for variant in &enum_def.variants {
            if let Some(value_expr) = &variant.value {
                current_tag = self.eval_const_discriminant(value_expr)?;
            }
            if variant.payload_type.is_none() && current_tag == tag {
                return Some(variant.name);
            }
            current_tag += 1;
        }

        None
    }

    fn anonymous_enum_variant_name_for_tag(
        &self,
        enum_def: &super::AnonymousEnum,
        tag: i128,
    ) -> Option<kernc_utils::SymbolId> {
        let mut current_tag = 0i128;
        for variant in &enum_def.variants {
            if let Some(value) = variant.explicit_value {
                current_tag = value;
            }
            if variant.payload_ty.is_none() && current_tag == tag {
                return Some(variant.name);
            }
            current_tag += 1;
        }
        None
    }

    fn eval_const_discriminant(&self, expr: &Expr) -> Option<i128> {
        match &expr.kind {
            ExprKind::Integer { value, .. } => Some(*value as i128),
            ExprKind::Unary {
                op: UnaryOperator::Negate,
                operand,
            } => self.eval_const_discriminant(operand)?.checked_neg(),
            ExprKind::Unary {
                op: UnaryOperator::BitwiseNot,
                operand,
            } => Some(!self.eval_const_discriminant(operand)?),
            ExprKind::Binary { lhs, op, rhs } => {
                let lhs = self.eval_const_discriminant(lhs)?;
                let rhs = self.eval_const_discriminant(rhs)?;
                match op {
                    BinaryOperator::Add => lhs.checked_add(rhs),
                    BinaryOperator::Subtract => lhs.checked_sub(rhs),
                    BinaryOperator::Multiply => lhs.checked_mul(rhs),
                    BinaryOperator::Divide => lhs.checked_div(rhs),
                    BinaryOperator::Modulo => lhs.checked_rem(rhs),
                    BinaryOperator::BitwiseAnd => Some(lhs & rhs),
                    BinaryOperator::BitwiseOr => Some(lhs | rhs),
                    BinaryOperator::BitwiseXor => Some(lhs ^ rhs),
                    BinaryOperator::ShiftLeft => {
                        let shift = u32::try_from(rhs).ok()?;
                        lhs.checked_shl(shift)
                    }
                    BinaryOperator::ShiftRight => {
                        let shift = u32::try_from(rhs).ok()?;
                        lhs.checked_shr(shift)
                    }
                    BinaryOperator::Equal
                    | BinaryOperator::NotEqual
                    | BinaryOperator::LessThan
                    | BinaryOperator::GreaterThan
                    | BinaryOperator::LessOrEqual
                    | BinaryOperator::GreaterOrEqual
                    | BinaryOperator::LogicalAnd
                    | BinaryOperator::LogicalOr => None,
                }
            }
            ExprKind::As { lhs, .. } => self.eval_const_discriminant(lhs),
            ExprKind::DataInit {
                literal: kernc_ast::DataLiteralKind::Scalar(inner),
                ..
            } => self.eval_const_discriminant(inner),
            _ => None,
        }
    }

    fn format_const_generic(&self, value: ConstGeneric) -> String {
        match value {
            ConstGeneric::Value(value) => self.format_const_generic_value(value),
            ConstGeneric::Param(symbol, _) => self.ctx.resolve(symbol).to_string(),
            ConstGeneric::Expr(id) => match self.ctx.type_registry.const_expr(id) {
                super::ConstExprKind::Unary { op, expr, .. } => {
                    let op_str = match op {
                        ConstExprUnaryOp::Negate => "-",
                        ConstExprUnaryOp::BitwiseNot => "~",
                    };
                    format!("({}{})", op_str, self.format_const_generic(*expr))
                }
                super::ConstExprKind::Binary { op, lhs, rhs, .. } => {
                    let op_str = match op {
                        ConstExprBinaryOp::Add => "+",
                        ConstExprBinaryOp::Subtract => "-",
                        ConstExprBinaryOp::Multiply => "*",
                        ConstExprBinaryOp::Divide => "/",
                        ConstExprBinaryOp::Modulo => "%",
                        ConstExprBinaryOp::BitwiseAnd => "&",
                        ConstExprBinaryOp::BitwiseOr => "|",
                        ConstExprBinaryOp::BitwiseXor => "^",
                        ConstExprBinaryOp::ShiftLeft => "<<",
                        ConstExprBinaryOp::ShiftRight => ">>",
                    };
                    format!(
                        "({} {} {})",
                        self.format_const_generic(*lhs),
                        op_str,
                        self.format_const_generic(*rhs)
                    )
                }
                super::ConstExprKind::Cast { expr, ty } => {
                    format!(
                        "({} as {})",
                        self.format_const_generic(*expr),
                        self.format(*ty)
                    )
                }
            },
            ConstGeneric::Error => "<const-error>".to_string(),
        }
    }

    fn format_generic_arg(&self, arg: GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.format(ty),
            GenericArg::Const(value) => self.format_const_generic(value),
        }
    }

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
                PrimitiveType::Never => "!".to_string(),
            },
            TypeKind::Simd { elem, lanes } => format!("{}x{}", self.format(*elem), lanes),
            TypeKind::Pointer { is_mut, elem } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("&{}{}", m, self.format(*elem))
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("^{}{}", m, self.format(*elem))
            }
            TypeKind::Slice { is_mut, elem } => {
                let m = if *is_mut { "mut " } else { "" };
                format!("&{}[{}]", m, self.format(*elem))
            }
            TypeKind::Range {
                start,
                end,
                is_inclusive,
            } => {
                let op = if *is_inclusive { "..=" } else { "..." };
                match (start, end) {
                    (Some(start), Some(end)) => {
                        format!("{}{}{}", self.format(*start), op, self.format(*end))
                    }
                    (Some(start), None) => format!("{}{}", self.format(*start), op),
                    (None, Some(end)) => format!("{}{}", op, self.format(*end)),
                    (None, None) => op.to_string(),
                }
            }
            TypeKind::Array { elem, len } => {
                format!(
                    "[{}]{}",
                    self.format_const_generic(*len),
                    self.format(*elem)
                )
            }
            TypeKind::ArrayInfer { elem } => format!("[_]{}", self.format(*elem)),
            TypeKind::TypeVar(vid) => format!("?T{}", vid),

            TypeKind::Def(def_id, generics) | TypeKind::Enum(def_id, generics) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<anonymous>");
                if generics.is_empty() {
                    name.to_string()
                } else {
                    let gen_strs: Vec<String> = generics
                        .iter()
                        .map(|g| self.format_generic_arg(*g))
                        .collect();
                    format!("{}[{}]", name, gen_strs.join(", "))
                }
            }
            TypeKind::TraitObject(def_id, generics, assoc_bindings) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                let name = def
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<anonymous>");
                if generics.is_empty() && assoc_bindings.is_empty() {
                    name.to_string()
                } else {
                    let mut parts = generics
                        .iter()
                        .map(|g| self.format_generic_arg(*g))
                        .collect::<Vec<_>>();
                    for (assoc_def_id, ty) in assoc_bindings {
                        let assoc_name = self.ctx.defs[assoc_def_id.0 as usize]
                            .name()
                            .map(|sym| self.ctx.resolve(sym))
                            .unwrap_or("<associated>");
                        parts.push(format!("{} = {}", assoc_name, self.format(*ty)));
                    }
                    format!("{}[{}]", name, parts.join(", "))
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
                    let gen_strs: Vec<String> = generics
                        .iter()
                        .map(|g| self.format_generic_arg(*g))
                        .collect();
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
                    let gen_strs: Vec<String> = generics
                        .iter()
                        .map(|g| self.format_generic_arg(*g))
                        .collect();
                    format!("{}[{}]", name, gen_strs.join(", "))
                }
            }
            TypeKind::Projection {
                target,
                trait_def_id,
                trait_args,
                assoc_def_id,
                assoc_args,
            } => {
                let trait_name = self.ctx.defs[trait_def_id.0 as usize]
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<trait>");
                let assoc_name = self.ctx.defs[assoc_def_id.0 as usize]
                    .name()
                    .map(|sym| self.ctx.resolve(sym))
                    .unwrap_or("<associated>");
                let mut out = self.format(*target);
                out.push('.');
                out.push_str(trait_name);
                if !trait_args.is_empty() {
                    let args = trait_args
                        .iter()
                        .map(|arg| self.format_generic_arg(*arg))
                        .collect::<Vec<_>>();
                    out.push('[');
                    out.push_str(&args.join(", "));
                    out.push(']');
                }
                out.push('.');
                out.push_str(assoc_name);
                if !assoc_args.is_empty() {
                    let args = assoc_args
                        .iter()
                        .map(|arg| self.format_generic_arg(*arg))
                        .collect::<Vec<_>>();
                    out.push('[');
                    out.push_str(&args.join(", "));
                    out.push(']');
                }
                out
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
                format!("&fn({}) {}", param_strs.join(", "), self.format(*ret))
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
                    let gen_strs: Vec<String> = generics
                        .iter()
                        .map(|g| self.format_generic_arg(*g))
                        .collect();
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
