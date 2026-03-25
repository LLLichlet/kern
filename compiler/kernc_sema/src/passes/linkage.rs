use std::collections::HashMap;
use crate::SemaContext;
use crate::def::Def;
use crate::ty::TypeId;
use kernc_ast::{AttributeKind, MetaItem, ExprKind};
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
            let (is_definition, is_extern, sig_ty, span, export_name_res) = {
                let def = &self.ctx.defs[i];
                match def {
                    Def::Function(f) => {
                        let name_str = self.ctx.resolve(f.name).to_string();
                        let export_res = self.extract_export_name(&name_str, f.is_extern, f.id.0, &f.attributes);
                        let sig_ty = f.resolved_sig.unwrap_or(TypeId::ERROR);
                        (f.body.is_some(), f.is_extern, sig_ty, f.span, export_res)
                    }
                    Def::Global(g) => {
                        let name_str = self.ctx.resolve(g.name).to_string();
                        let export_res = self.extract_export_name(&name_str, g.is_extern, g.id.0, &g.attributes);
                        let sig_ty = self.ctx.node_types.get(&g.value.id).copied().unwrap_or(TypeId::ERROR);
                        (!g.is_extern, g.is_extern, sig_ty, g.span, export_res)
                    }
                    _ => continue, 
                }
            };

            // 如果之前的阶段类型推导失败，跳过
            if sig_ty == TypeId::ERROR { continue; }

            let export_name = match export_name_res {
                Ok(name) => name,
                Err(err_span) => {
                    self.ctx.struct_error(err_span, "`export_name` requires a string literal")
                        .with_hint(r#"example: #[export_name("_start")]"#)
                        .emit();
                    continue; // 遇到非法属性，直接跳过当前符号的链接检查
                }
            };

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

    /// 辅助方法：提取符号在 LLVM 里的最终导出名 (Linkage Name)
    fn extract_export_name(
        &self, 
        default_name: &str, 
        is_extern: bool,
        def_id_num: u32,
        attributes: &[kernc_ast::Attribute]
    ) -> Result<String, Span> {
        for attr in attributes {
            if let AttributeKind::Meta(items) = &attr.kind {
                for item in items {
                    if let MetaItem::Call(sym_id, arg_expr) = item {
                        let name_str = self.ctx.resolve(*sym_id);
                        if name_str == "export_name" {
                            if let ExprKind::String(ref s) = arg_expr.kind {
                                return Ok(s.clone());
                            } else {
                                return Err(arg_expr.span); 
                            }
                        }
                    }
                }
            }
        }

        // if default_name == "main" {
        //     return Ok(default_name.to_string());
        // }

        if is_extern {
            return Ok(default_name.to_string());
        }

        Ok(format!("{}..{}", default_name, def_id_num))
    }
}