use std::collections::HashMap;

use kernc_ast as ast;
use kernc_mast::*;
use kernc_sema::SemaContext;
use kernc_sema::def::{EnumDef, Def, DefId};
use kernc_sema::ty::TypeId;
use kernc_utils::SymbolId;

pub(crate) mod expr;
pub(crate) mod mono;
pub(crate) mod vtable;

pub struct Lowerer<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
    pub module: MastModule,

    pub(crate) mono_cache: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pub(crate) next_mono_id: u32,
    pub(crate) defer_stack: Vec<Vec<MastExpr>>,
    pub(crate) global_map: HashMap<DefId, MonoId>,
    pub(crate) global_symbol_map: HashMap<SymbolId, MonoId>,
    pub(crate) vtable_cache: HashMap<(TypeId, TypeId), MonoId>,
    pub(crate) local_types: Vec<HashMap<SymbolId, (TypeId, bool)>>,
    pub(crate) local_statics: Vec<HashMap<SymbolId, MonoId>>,
    pub(crate) loop_frames: Vec<usize>,
    pub(crate) adt_union_map: HashMap<MonoId, MonoId>,
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
            },
            mono_cache: HashMap::new(),
            next_mono_id: 1,
            defer_stack: Vec::new(),
            global_map: HashMap::new(),
            global_symbol_map: HashMap::new(),
            vtable_cache: HashMap::new(),
            local_types: Vec::new(),
            local_statics: Vec::new(),
            loop_frames: Vec::new(),
            adt_union_map: HashMap::new(),
        }
    }

    fn new_mono_id(&mut self) -> MonoId {
        let id = self.next_mono_id;
        self.next_mono_id += 1;
        MonoId(id)
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
                    // 内置函数没有物理实体，直接跳过，不进入 MAST
                    if f.is_intrinsic {
                        continue;
                    }
                    // 检查函数自身和其父级（Impl块）是否包含泛型
                    // 只有自己没泛型，且爹也没泛型的函数，才是真正的“自由函数”，才能在此刻被实例化
                    let mut is_generic = !f.generics.is_empty();
                    if let Some(parent_id) = f.parent {
                        if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                            if !impl_def.generics.is_empty() {
                                is_generic = true;
                            }
                        }
                    }

                    if !is_generic {
                        self.instantiate_function(id, &[]);
                    }
                }
                Def::Global(g) => self.lower_global(&g),
                _ => {}
            }
        }

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
}
