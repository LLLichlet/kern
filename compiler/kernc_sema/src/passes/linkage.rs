use std::collections::HashMap;
use crate::SemaContext;
use crate::def::{Def, DefId};
use crate::ty::TypeId;
use kernc_utils::Span;

pub struct LinkageChecker<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
}

impl<'a, 'ctx> LinkageChecker<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn check_all(&mut self) {
        // 记录: 导出名 -> (是否是具体定义, 签名类型, 是否有extern修饰, 声明位置)
        let mut symbols: HashMap<String, (bool, TypeId, bool, Span)> = HashMap::new();

        for i in 0..self.ctx.defs.len() {
            let def_id = DefId(i as u32);
            let def = self.ctx.defs[i].clone(); // Clone 解决借用冲突

            let (is_definition, is_extern, sig_ty, span) = match def {
                Def::Function(f) => {
                    // 检查是否是泛型函数（自身带泛型，或身处泛型 impl 块中）
                    let mut is_generic = !f.generics.is_empty();
                    if let Some(parent_id) = f.parent {
                        if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                            if !impl_def.generics.is_empty() {
                                is_generic = true;
                            }
                        }
                    }

                    // 泛型模板不产生实际的 C ABI 链接符号，直接跳过
                    if is_generic {
                        continue;
                    }

                    let sig_ty = f.resolved_sig.unwrap_or(TypeId::ERROR);
                    (f.body.is_some(), f.is_extern, sig_ty, f.span)
                }
                Def::Global(g) => {
                    let sig_ty = self.ctx.node_types.get(&g.value.id).copied().unwrap_or(TypeId::ERROR);
                    (!g.is_extern, g.is_extern, sig_ty, g.span)
                }
                _ => continue,
            };

            // 如果之前的阶段类型推导失败，跳过
            if sig_ty == TypeId::ERROR { continue; }

            let export_name = self.ctx.get_export_name(def_id, &[]);

            if let Some((prev_is_def, prev_sig_ty, prev_is_extern, prev_span)) = symbols.get(&export_name) {
                if *prev_sig_ty != sig_ty {
                    let expected_str = self.ctx.ty_to_string(*prev_sig_ty);
                    let found_str = self.ctx.ty_to_string(sig_ty);
                    
                    self.ctx.struct_error(span, format!("linkage signature mismatch for symbol `{}`", export_name))
                        .with_hint(format!("expected signature: {}", expected_str))
                        .with_hint(format!("found signature:    {}", found_str))
                        .with_span_label(*prev_span, "previously declared/defined here")
                        .emit();
                } else if is_definition && *prev_is_def {
                    self.ctx.struct_error(span, format!("duplicate definition of symbol `{}`", export_name))
                        .with_span_label(*prev_span, "first definition was here")
                        .emit();
                } else if is_definition && !is_extern && *prev_is_extern {
                    self.ctx.struct_error(span, format!("definition of `{}` must be explicitly marked as `extern`", export_name))
                        .with_hint("it matches an external C-ABI declaration from another module")
                        .with_span_label(*prev_span, "external declaration was here")
                        .emit();
                }
                
                if is_definition && !*prev_is_def {
                    symbols.insert(export_name, (is_definition, sig_ty, is_extern, span));
                }
            } else {
                symbols.insert(export_name, (is_definition, sig_ty, is_extern, span));
            }
        }
    }
}