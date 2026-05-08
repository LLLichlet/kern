use crate::SemaContext;
use crate::def::Def;
use crate::ty::TypeId;
use kernc_utils::Span;
use std::collections::HashMap;

pub struct LinkageChecker<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

impl<'a, 'ctx> LinkageChecker<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn check_all(&mut self) {
        // Track: export name -> (is concrete definition, signature, extern flag, declaration span).
        let mut symbols: HashMap<String, (bool, TypeId, bool, Span)> = HashMap::new();

        for def_id in self.ctx.defs.ids().collect::<Vec<_>>() {
            let def = self.ctx.defs[def_id.0 as usize].clone(); // Clone to avoid borrow conflicts.

            let (is_definition, is_extern, sig_ty, span) = match def {
                Def::Function(f) => {
                    self.check_attribute_surface(&f.attributes);

                    // Check whether this is a generic template, either directly or through its impl.
                    let mut is_generic = !f.generics.is_empty();
                    if let Some(parent_id) = f.parent
                        && let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize]
                        && !impl_def.generics.is_empty()
                    {
                        is_generic = true;
                    }

                    // Generic templates do not produce concrete C ABI symbols.
                    if is_generic {
                        continue;
                    }

                    let sig_ty = f.resolved_sig.unwrap_or(TypeId::ERROR);
                    (f.body.is_some(), f.is_extern, sig_ty, f.span)
                }
                Def::Global(g) => {
                    self.check_attribute_surface(&g.attributes);
                    let sig_ty = self.ctx.node_type_or_error(g.value.id);
                    (!g.is_extern, g.is_extern, sig_ty, g.span)
                }
                _ => continue,
            };

            // Skip items whose types failed to resolve in earlier passes.
            if sig_ty == TypeId::ERROR {
                continue;
            }

            let export_name = self.ctx.get_export_name(def_id, &[]);

            if let Some((prev_is_def, prev_sig_ty, prev_is_extern, prev_span)) =
                symbols.get(&export_name)
            {
                if *prev_sig_ty != sig_ty {
                    let expected_str = self.ctx.ty_to_string(*prev_sig_ty);
                    let found_str = self.ctx.ty_to_string(sig_ty);

                    self.ctx
                        .struct_error(
                            span,
                            format!("linkage signature mismatch for symbol `{}`", export_name),
                        )
                        .with_hint(format!("expected signature: {}", expected_str))
                        .with_hint(format!("found signature:    {}", found_str))
                        .with_span_label(*prev_span, "previously declared/defined here")
                        .emit();
                } else if is_definition && *prev_is_def {
                    self.ctx
                        .struct_error(
                            span,
                            format!("duplicate definition of symbol `{}`", export_name),
                        )
                        .with_span_label(*prev_span, "first definition was here")
                        .emit();
                } else if is_definition && !is_extern && *prev_is_extern {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "definition of `{}` must be explicitly marked as `extern`",
                                export_name
                            ),
                        )
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

    fn check_attribute_surface(&mut self, attributes: &[kernc_ast::Attribute]) {
        for attr in attributes {
            let kernc_ast::AttributeKind::Meta(items) = &attr.kind else {
                continue;
            };

            for item in items {
                match item {
                    kernc_ast::MetaItem::Marker(name) => {
                        if self.ctx.resolve(*name) == "inline_always" {
                            self.ctx
                                .struct_error(attr.span, "`#[inline_always]` is not supported")
                                .with_hint("use `#[inline]` for forced inlining")
                                .emit();
                        }
                    }
                    kernc_ast::MetaItem::Call(name, _) => match self.ctx.resolve(*name) {
                        "inline" => {
                            self.ctx
                                .struct_error(attr.span, "`#[inline(...)]` is not supported")
                                .with_hint("use marker attributes: `#[inline]` or `#[noinline]`")
                                .emit();
                        }
                        "noinline" => {
                            self.ctx
                                .struct_error(attr.span, "`#[noinline(...)]` is not supported")
                                .with_hint("use marker attributes: `#[inline]` or `#[noinline]`")
                                .emit();
                        }
                        "retain" => {
                            self.ctx
                                .struct_error(attr.span, "`#[retain(...)]` is not supported")
                                .with_hint("use the marker attribute: `#[retain]`")
                                .emit();
                        }
                        _ => {}
                    },
                }
            }
        }
    }
}
