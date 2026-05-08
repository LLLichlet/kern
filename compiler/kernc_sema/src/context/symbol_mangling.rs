use super::*;

impl<'a> SemaContext<'a> {
    fn def_source_name(&self, def_id: DefId) -> String {
        self.defs[def_id.0 as usize]
            .name()
            .map(|name_sym| self.resolve(name_sym).to_string())
            .unwrap_or_else(|| format!("AnonDef{}", def_id.0))
    }

    fn def_parent_for_path(&self, def_id: DefId) -> Option<DefId> {
        match &self.defs[def_id.0 as usize] {
            Def::Module(module) => module.parent,
            Def::Function(function) => function.parent,
            Def::Global(global) => global.parent,
            Def::Impl(impl_def) => impl_def.parent_module,
            Def::Struct(_)
            | Def::Union(_)
            | Def::Enum(_)
            | Def::Trait(_)
            | Def::AssociatedType(_)
            | Def::TypeAlias(_) => self.def_parent_module(def_id),
        }
    }

    fn parent_path_components(&self, mut parent_id: Option<DefId>) -> Vec<String> {
        let mut path_components = Vec::new();
        while let Some(def_id) = parent_id {
            match &self.defs[def_id.0 as usize] {
                Def::Module(module) => {
                    path_components.push(self.resolve(module.name).to_string());
                    parent_id = module.parent;
                }
                Def::Impl(impl_def) => {
                    let target_ty = self.node_type_or_error(impl_def.target_type.id);
                    path_components.push(self.mangle_type(target_ty));
                    if let Some(trait_ty) = &impl_def.trait_type {
                        let trait_ty = self.node_type_or_error(trait_ty.id);
                        path_components.push(self.mangle_type(trait_ty));
                    }
                    parent_id = impl_def.parent_module;
                }
                _ => break,
            }
        }
        path_components
    }

    fn def_qualified_name(&self, def_id: DefId) -> String {
        let base_name = self.def_source_name(def_id);
        let path_components = self.parent_path_components(self.def_parent_for_path(def_id));
        if path_components.is_empty() {
            return base_name;
        }

        let mut qualified = String::from("Q");
        for component in path_components.into_iter().rev() {
            qualified.push_str(&format!("{}{}", component.len(), component));
        }
        qualified.push_str(&format!("{}{}", base_name.len(), base_name));
        qualified.push('E');
        qualified
    }

    fn mangle_const_generic(&self, value: crate::ty::ConstGeneric) -> String {
        match value {
            crate::ty::ConstGeneric::Value(value) => {
                let payload = match value.kind {
                    crate::ty::ConstGenericValueKind::Int(value) => format!("i{}", value),
                    crate::ty::ConstGenericValueKind::Bool(value) => {
                        if value {
                            "b1".to_string()
                        } else {
                            "b0".to_string()
                        }
                    }
                };
                format!("C{}{}", self.mangle_type(value.ty), payload)
            }
            crate::ty::ConstGeneric::Param(symbol, ty) => {
                format!("P{}{}", self.mangle_type(ty), symbol.0)
            }
            crate::ty::ConstGeneric::Expr(id) => match self.type_registry.const_expr(id) {
                crate::ty::ConstExprKind::Unary { op, expr, ty } => {
                    let op_code = match op {
                        crate::ty::ConstExprUnaryOp::Negate => "neg",
                        crate::ty::ConstExprUnaryOp::BitwiseNot => "not",
                    };
                    format!(
                        "Eu{}{}{}",
                        op_code,
                        self.mangle_type(*ty),
                        self.mangle_const_generic(*expr)
                    )
                }
                crate::ty::ConstExprKind::Binary { op, lhs, rhs, ty } => {
                    let op_code = match op {
                        crate::ty::ConstExprBinaryOp::Add => "add",
                        crate::ty::ConstExprBinaryOp::Subtract => "sub",
                        crate::ty::ConstExprBinaryOp::Multiply => "mul",
                        crate::ty::ConstExprBinaryOp::Divide => "div",
                        crate::ty::ConstExprBinaryOp::Modulo => "mod",
                        crate::ty::ConstExprBinaryOp::BitwiseAnd => "and",
                        crate::ty::ConstExprBinaryOp::BitwiseOr => "or",
                        crate::ty::ConstExprBinaryOp::BitwiseXor => "xor",
                        crate::ty::ConstExprBinaryOp::ShiftLeft => "shl",
                        crate::ty::ConstExprBinaryOp::ShiftRight => "shr",
                    };
                    format!(
                        "Eb{}{}{}{}",
                        op_code,
                        self.mangle_type(*ty),
                        self.mangle_const_generic(*lhs),
                        self.mangle_const_generic(*rhs)
                    )
                }
                crate::ty::ConstExprKind::Cast { expr, ty } => {
                    format!(
                        "Ec{}{}",
                        self.mangle_type(*ty),
                        self.mangle_const_generic(*expr)
                    )
                }
            },
            crate::ty::ConstGeneric::Error => "Cerror".to_string(),
        }
    }

    fn mangle_generic_arg(&self, arg: crate::ty::GenericArg) -> String {
        match arg {
            crate::ty::GenericArg::Type(ty) => self.mangle_type(ty),
            crate::ty::GenericArg::Const(value) => self.mangle_const_generic(value),
        }
    }

    /// Generate a deterministic mangling suffix for a semantic type.
    pub fn mangle_type(&self, ty: TypeId) -> String {
        let norm_ty = self.type_registry.normalize(ty);
        match self.type_registry.get(norm_ty).clone() {
            crate::ty::TypeKind::Primitive(p) => match p {
                crate::ty::PrimitiveType::Void => "void".to_string(),
                crate::ty::PrimitiveType::Bool => "bool".to_string(),
                crate::ty::PrimitiveType::I8 => "i8".to_string(),
                crate::ty::PrimitiveType::I16 => "i16".to_string(),
                crate::ty::PrimitiveType::I32 => "i32".to_string(),
                crate::ty::PrimitiveType::I64 => "i64".to_string(),
                crate::ty::PrimitiveType::I128 => "i128".to_string(),
                crate::ty::PrimitiveType::ISize => "isize".to_string(),
                crate::ty::PrimitiveType::U8 => "u8".to_string(),
                crate::ty::PrimitiveType::U16 => "u16".to_string(),
                crate::ty::PrimitiveType::U32 => "u32".to_string(),
                crate::ty::PrimitiveType::U64 => "u64".to_string(),
                crate::ty::PrimitiveType::U128 => "u128".to_string(),
                crate::ty::PrimitiveType::USize => "usize".to_string(),
                crate::ty::PrimitiveType::F32 => "f32".to_string(),
                crate::ty::PrimitiveType::F64 => "f64".to_string(),
                crate::ty::PrimitiveType::Never => "never".to_string(),
            },
            crate::ty::TypeKind::Simd { elem, lanes } => {
                let inner = self.mangle_type(elem);
                format!("Simd{}x{}", inner, lanes)
            }
            crate::ty::TypeKind::Pointer { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("Pmut{}", inner)
                } else {
                    format!("P{}", inner)
                }
            }
            crate::ty::TypeKind::VolatilePtr { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("Vmut{}", inner)
                } else {
                    format!("V{}", inner)
                }
            }
            crate::ty::TypeKind::Slice { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("S{}_mut", inner)
                } else {
                    format!("S{}", inner)
                }
            }
            crate::ty::TypeKind::Array { elem, len } => {
                let inner = self.mangle_type(elem);
                format!("A{}{}", len, inner)
            }
            crate::ty::TypeKind::Def(def_id, gen_args)
            | crate::ty::TypeKind::Enum(def_id, gen_args)
            | crate::ty::TypeKind::TraitObject(def_id, gen_args, _) => {
                let base_name = self.def_qualified_name(def_id);

                if gen_args.is_empty() {
                    base_name
                } else {
                    let mut s = format!("{}I", base_name);
                    for arg in gen_args {
                        let arg_mangled = self.mangle_generic_arg(arg);
                        s.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
                    }
                    s.push('E');
                    s
                }
            }
            crate::ty::TypeKind::Function { params, ret, .. }
            | crate::ty::TypeKind::ClosureInterface { params, ret } => {
                let mut s = String::from("F");
                for p in params {
                    let p_str = self.mangle_type(p);
                    s.push_str(&format!("{}{}", p_str.len(), p_str));
                }
                s.push('R');
                let r_str = self.mangle_type(ret);
                s.push_str(&format!("{}{}", r_str.len(), r_str));
                s
            }
            crate::ty::TypeKind::FnDef(def_id, gen_args) => {
                let base_name = self.def_qualified_name(def_id);
                if gen_args.is_empty() {
                    base_name
                } else {
                    let mut s = format!("{}I", base_name);
                    for arg in gen_args {
                        let arg_mangled = self.mangle_generic_arg(arg);
                        s.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
                    }
                    s.push('E');
                    s
                }
            }
            crate::ty::TypeKind::AnonymousState {
                closure_node_id, ..
            } => {
                format!("ClosureState{}", closure_node_id.0)
            }
            crate::ty::TypeKind::AnonymousStruct(is_extern, fields) => {
                let mut s = if is_extern {
                    String::from("EStr")
                } else {
                    String::from("AStr")
                };
                for f in fields {
                    let name_str = self.resolve(f.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    let ty_str = self.mangle_type(f.ty);
                    s.push_str(&format!("{}{}", ty_str.len(), ty_str));
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousUnion(is_extern, fields) => {
                let mut s = if is_extern {
                    String::from("EUni")
                } else {
                    String::from("AUni")
                };
                for f in fields {
                    let name_str = self.resolve(f.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    let ty_str = self.mangle_type(f.ty);
                    s.push_str(&format!("{}{}", ty_str.len(), ty_str));
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousEnum(enum_def) => {
                let mut s = String::from("AEnum");
                if let Some(backing_ty) = enum_def.backing_ty {
                    let backing = self.mangle_type(backing_ty);
                    s.push_str(&format!("B{}{}", backing.len(), backing));
                }
                for variant in &enum_def.variants {
                    let name_str = self.resolve(variant.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    if let Some(payload_ty) = variant.payload_ty {
                        let payload = self.mangle_type(payload_ty);
                        s.push_str(&format!("P{}{}", payload.len(), payload));
                    } else {
                        s.push('N');
                    }
                    if let Some(value) = variant.explicit_value {
                        s.push_str(&format!("V{}", value));
                    }
                    s.push('_');
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousEnumPayload(enum_ty) => {
                let inner = self.mangle_type(enum_ty);
                format!("AEnumPayload{}{}", inner.len(), inner)
            }
            _ => "unknown".to_string(),
        }
    }

    /// Compute the final exported linker symbol for a definition instance.
    pub fn get_export_name_for_generic_args(
        &self,
        def_id: DefId,
        args: &[crate::ty::GenericArg],
    ) -> String {
        let def = &self.defs[def_id.0 as usize];
        let name_str = self.def_source_name(def_id);

        let empty_attrs: &[kernc_ast::Attribute] = &[];
        let (is_extern, attrs) = match def {
            Def::Function(f) => (f.is_extern, f.attributes.as_slice()),
            Def::Global(g) => (g.is_extern, g.attributes.as_slice()),
            Def::Struct(s) => (s.is_extern, s.attributes.as_slice()),
            Def::Enum(_) => (false, empty_attrs),
            Def::Union(u) => (u.is_extern, empty_attrs),
            _ => return name_str,
        };
        let parent_id = self.def_parent_for_path(def_id);

        if args.is_empty() {
            for attr in attrs {
                if let kernc_ast::AttributeKind::Meta(items) = &attr.kind {
                    for item in items {
                        if let kernc_ast::MetaItem::Call(sym_id, arg_expr) = item
                            && self.resolve(*sym_id) == "export_name"
                            && let kernc_ast::ExprKind::String(ref s) = arg_expr.kind
                        {
                            return s.clone();
                        }
                    }
                }
            }
        }

        if is_extern && args.is_empty() {
            return name_str;
        }

        let mut mangled = String::from("_K");
        for comp in self.parent_path_components(parent_id).into_iter().rev() {
            mangled.push_str(&format!("{}{}", comp.len(), comp));
        }

        mangled.push_str(&format!("{}{}", name_str.len(), name_str));

        if !args.is_empty() {
            mangled.push('I');
            for &arg in args {
                let arg_mangled = self.mangle_generic_arg(arg);
                mangled.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
            }
            mangled.push('E');
        }

        mangled
    }

    pub fn get_export_name(&self, def_id: DefId, args: &[TypeId]) -> String {
        self.get_export_name_for_generic_args(
            def_id,
            &crate::ty::wrap_type_args(args.iter().copied()),
        )
    }
}
