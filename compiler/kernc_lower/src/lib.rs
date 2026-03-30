use std::collections::HashMap;

use kernc_ast as ast;
use kernc_mast::*;
use kernc_sema::SemaContext;
use kernc_sema::checker::Substituter;
use kernc_sema::def::{Def, DefId, EnumDef, FunctionDef};
use kernc_sema::scope::ScopeId;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{NodeId, Span, SymbolId};

pub(crate) mod expr;
pub(crate) mod mono;
pub(crate) mod vtable;

pub struct Lowerer<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    module: MastModule,

    pub(crate) mono_cache: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pub(crate) pure_enum_tag_map: HashMap<(DefId, Vec<TypeId>), TypeId>,
    pub(crate) next_mono_id: u32,
    pub(crate) defer_stack: Vec<Vec<MastExpr>>,
    pub(crate) global_map: HashMap<DefId, MonoId>,
    pub(crate) global_symbol_map: HashMap<SymbolId, MonoId>,
    pub(crate) vtable_cache: HashMap<(TypeId, TypeId), MonoId>,
    pub(crate) local_types: Vec<HashMap<SymbolId, (TypeId, bool)>>,
    pub(crate) local_statics: Vec<HashMap<SymbolId, MonoId>>,
    pub(crate) loop_frames: Vec<usize>,
    pub(crate) adt_union_map: HashMap<MonoId, MonoId>,
    pub(crate) closure_fn_map: HashMap<NodeId, MonoId>,
    pub(crate) anon_struct_cache: HashMap<TypeId, MonoId>,
    pub(crate) anon_union_cache: HashMap<TypeId, MonoId>,
    pub(crate) anon_enum_cache: HashMap<TypeId, MonoId>,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            module: MastModule {
                name: "kern_out".to_string(),
                structs: Vec::new(),
                globals: Vec::new(),
                functions: Vec::new(),
                def_mono_map: HashMap::new(),
                pure_enum_tag_map: HashMap::new(),
                adt_union_map: HashMap::new(),
                anon_struct_map: HashMap::new(),
                anon_union_map: HashMap::new(),
                anon_enum_map: HashMap::new(),
            },
            mono_cache: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            next_mono_id: 1,
            defer_stack: Vec::new(),
            global_map: HashMap::new(),
            global_symbol_map: HashMap::new(),
            vtable_cache: HashMap::new(),
            local_types: Vec::new(),
            local_statics: Vec::new(),
            loop_frames: Vec::new(),
            adt_union_map: HashMap::new(),
            closure_fn_map: HashMap::new(),
            anon_struct_cache: HashMap::new(),
            anon_union_cache: HashMap::new(),
            anon_enum_cache: HashMap::new(),
        }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    fn new_mono_id(&mut self) -> MonoId {
        let id = self.next_mono_id;
        self.next_mono_id += 1;
        MonoId(id)
    }

    fn function_requires_runtime_body(&self, f: &kernc_sema::def::FunctionDef) -> bool {
        if !f.is_imported {
            return true;
        }
        if !f.generics.is_empty() {
            return true;
        }
        if let Some(parent_id) = f.parent
            && let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize]
        {
            return !impl_def.generics.is_empty();
        }
        false
    }

    pub(crate) fn function_owner_scope(&self, f: &FunctionDef) -> Option<ScopeId> {
        let parent_id = f.parent?;
        match &self.ctx.defs[parent_id.0 as usize] {
            Def::Module(module) => Some(module.scope_id),
            Def::Impl(impl_def) => {
                let module_id = impl_def.parent_module?;
                match &self.ctx.defs[module_id.0 as usize] {
                    Def::Module(module) => Some(module.scope_id),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// 降级入口：寻找所有非泛型的根节点向下递归单态化
    pub fn lower_all(&mut self) -> MastModule {
        let def_ids: Vec<_> = (0..self.ctx.defs.len()).map(|i| DefId(i as u32)).collect();

        // Phase 1: 预分配全局变量的 MonoId
        for &id in &def_ids {
            let global_name = if let Def::Global(g) = &self.ctx.defs[id.0 as usize] {
                Some(g.name)
            } else {
                None
            };

            if let Some(name) = global_name {
                let mono_id = self.new_mono_id();
                self.global_map.insert(id, mono_id);
                // 预注册顶层全局变量的名字
                self.global_symbol_map.insert(name, mono_id);
            }
        }

        // Phase 2: 执行真正的实体降级
        for id in def_ids {
            let def = self.ctx.defs[id.0 as usize].clone();
            match def {
                Def::Function(f) => {
                    if f.is_imported {
                        continue;
                    }
                    // 内置函数没有物理实体，直接跳过，不进入 MAST
                    if f.is_intrinsic {
                        continue;
                    }
                    // 检查函数自身和其父级（Impl块）是否包含泛型
                    // 只有自己没泛型，且爹也没泛型的函数，才是真正的“自由函数”，才能在此刻被实例化
                    let mut is_generic = !f.generics.is_empty();
                    if let Some(parent_id) = f.parent
                        && let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize]
                        && !impl_def.generics.is_empty()
                    {
                        is_generic = true;
                    }

                    if !is_generic {
                        self.instantiate_function(id, &[]);
                    }
                }
                Def::Global(g) => {
                    if !g.is_imported {
                        self.lower_global(&g);
                    }
                }
                _ => {}
            }
        }

        self.module.def_mono_map = self.mono_cache.clone();
        self.module.pure_enum_tag_map = self.pure_enum_tag_map.clone();
        self.module.adt_union_map = self.adt_union_map.clone();
        self.module.anon_struct_map = self.anon_struct_cache.clone();
        self.module.anon_union_map = self.anon_union_cache.clone();
        self.module.anon_enum_map = self.anon_enum_cache.clone();

        self.module.clone()
    }

    pub(crate) fn extract_meta_items(&self, attrs: &[ast::Attribute]) -> Vec<ast::MetaItem> {
        let mut meta = Vec::new();
        for attr in attrs {
            if let ast::AttributeKind::Meta(items) = &attr.kind {
                meta.extend(items.clone());
            }
        }
        meta
    }

    /// 纯数据探测器：如果所有的变体都没有负载，那么它在内存中就完全等价于一个整数。
    pub(crate) fn is_pure_enum(&self, def: &EnumDef) -> bool {
        def.variants.iter().all(|v| v.payload_type.is_none())
    }

    pub(crate) fn record_pure_enum_tag_ty(&mut self, def_id: DefId, args: &[TypeId]) -> TypeId {
        let key = (def_id, args.to_vec());
        if let Some(&tag_ty) = self.pure_enum_tag_map.get(&key) {
            return tag_ty;
        }

        let Some(Def::Enum(def)) = self.ctx.defs.get(def_id.0 as usize).cloned() else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): DefId {} is not an enum while recording pure enum representation.",
                    def_id.0
                ),
            );
            return TypeId::ERROR;
        };

        let raw_tag_ty = def.backing_type.as_ref().map_or(TypeId::U32, |backing_ty| {
            self.ctx
                .node_types
                .get(&backing_ty.id)
                .copied()
                .unwrap_or(TypeId::U32)
        });

        let tag_ty = if def.generics.is_empty() || args.is_empty() {
            raw_tag_ty
        } else {
            let mut subst_map = HashMap::new();
            for (param, arg) in def.generics.iter().zip(args.iter().copied()) {
                subst_map.insert(param.name, arg);
            }
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
            subst.substitute(raw_tag_ty)
        };

        self.pure_enum_tag_map.insert(key, tag_ty);
        tag_ty
    }

    pub(crate) fn track_pure_enum_repr_in_type(&mut self, ty: TypeId) {
        let norm_ty = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm_ty).clone() {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Array { elem, .. }
            | TypeKind::ArrayInfer { elem, .. } => self.track_pure_enum_repr_in_type(elem),
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                for param in params {
                    self.track_pure_enum_repr_in_type(param);
                }
                self.track_pure_enum_repr_in_type(ret);
            }
            TypeKind::Def(def_id, args) => {
                self.instantiate_struct(def_id, &args);
            }
            TypeKind::Enum(def_id, args) => {
                if let Some(Def::Enum(def)) = self.ctx.defs.get(def_id.0 as usize).cloned() {
                    if self.is_pure_enum(&def) {
                        self.record_pure_enum_tag_ty(def_id, &args);
                    } else {
                        self.instantiate_data(def_id, &args);
                    }
                }
            }
            TypeKind::Alias(_, inner) => self.track_pure_enum_repr_in_type(inner),
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => {
                for field in fields {
                    self.track_pure_enum_repr_in_type(field.ty);
                }
            }
            _ => {}
        }
    }

    /// 通过闭包结构体的 AST 节点 ID，获取对应的执行包装函数的 MonoId
    pub(crate) fn get_closure_func_mono_id(&mut self, closure_node_id: NodeId) -> MonoId {
        match self.closure_fn_map.get(&closure_node_id) {
            Some(&id) => id,
            None => {
                // 如果找不到，说明存在编译器内部错误 (比如 Sema 生成了匿名状态，但 Lowering 还没处理到那个闭包表达式就被提前引用了)
                self.ctx.emit_ice(
                    Span::default(),
                    format!("Kern ICE (Lowering): Attempted to resolve a closure function ID before the closure expression (NodeId {}) was lowered.", closure_node_id.0)
                );
                let placeholder = self.new_mono_id();
                self.closure_fn_map.insert(closure_node_id, placeholder);
                placeholder
            }
        }
    }

    pub(crate) fn global_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.ctx.defs.iter().find_map(|def| {
            let Def::Module(module) = def else {
                return None;
            };

            if module.items.contains(&def_id) {
                Some(module.scope_id)
            } else {
                None
            }
        })
    }
}
