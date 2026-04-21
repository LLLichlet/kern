use super::ExprChecker;
use crate::checker::{ConstEvaluator, ConstValue};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::Span;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(super) fn check_asm_call(&mut self, args: &[Expr], span: Span) -> TypeId {
        if args.len() != 1 {
            self.ctx
                .struct_error(span, "`@asm` expects exactly one anonymous struct argument")
                .with_hint("example: `@asm(.{ asm: \"nop\", volatile: true })`")
                .emit();
            return TypeId::ERROR;
        }

        let config_arg = &args[0];
        let fields = match &config_arg.kind {
            ExprKind::DataInit {
                literal: ast::DataLiteralKind::Struct(f),
                type_node: None,
            } => f,
            _ => {
                self.ctx
                    .struct_error(
                        config_arg.span,
                        "`@asm` argument must be an untyped anonymous struct `.{ ... }`",
                    )
                    .emit();
                self.check_expr(config_arg, None);
                return TypeId::ERROR;
            }
        };

        let mut has_asm = false;

        for field in fields {
            let field_name = self.ctx.resolve(field.name).to_string();
            match field_name.as_str() {
                "asm" => {
                    has_asm = true;
                    match &field.value.kind {
                        ExprKind::String(_) => {
                            self.check_expr(&field.value, None);
                        }
                        _ => {
                            let mut diag = self.ctx.struct_error(
                                field.value.span,
                                "`asm` template must be a string literal",
                            );
                            if matches!(
                                field.value.kind,
                                ExprKind::DataInit {
                                    literal: ast::DataLiteralKind::Array(_),
                                    ..
                                }
                            ) {
                                diag = diag.with_hint(
                                    "use one string literal instead; for multiple lines, use Kern multiline strings beginning with `\\\\`",
                                );
                            }
                            diag.emit();
                        }
                    }
                }
                "outputs" | "inputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg_field in regs {
                            let val_ty = self.check_expr(&reg_field.value, None);
                            let val_ty_str = self.ctx.ty_to_string(val_ty);

                            if field_name == "outputs"
                                && val_ty != TypeId::ERROR
                                && !self.is_mut_pointer(val_ty)
                            {
                                self.ctx
                                    .struct_error(
                                        reg_field.value.span,
                                        "inline assembly outputs must be bound to mutable pointers (e.g., `status..&`)",
                                    )
                                    .with_hint(format!("type found: {}", val_ty_str))
                                    .emit();
                            }
                        }
                    } else {
                        self.ctx
                            .struct_error(
                                field.value.span,
                                format!(
                                    "`{}` must be an anonymous struct mapping registers to variables",
                                    field_name
                                ),
                            )
                            .emit();
                        self.check_expr(&field.value, None);
                    }
                }
                "clobbers" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Array(clobbers),
                        ..
                    } = &field.value.kind
                    {
                        for c in clobbers {
                            if !matches!(c.kind, ExprKind::String(_)) {
                                self.ctx
                                    .struct_error(
                                        c.span,
                                        "clobbers must be a list of string literals (e.g., `.{ \"memory\", \"cc\" }`)",
                                    )
                                    .emit();
                            }
                            self.check_expr(c, None);
                        }
                    } else {
                        self.ctx
                            .struct_error(
                                field.value.span,
                                "`clobbers` must be a slice/array of strings",
                            )
                            .emit();
                        self.check_expr(&field.value, None);
                    }
                }
                "volatile" => {
                    let ty = self.check_expr(&field.value, Some(TypeId::BOOL));
                    self.check_coercion(&field.value, TypeId::BOOL, ty);
                    if self.resolve_tv(ty) == TypeId::BOOL {
                        let mut evaluator = ConstEvaluator::new(self.ctx);
                        match evaluator.eval_const_value(&field.value) {
                            Ok(ConstValue::Bool(_)) => {}
                            Ok(_) => {
                                self.ctx
                                    .struct_error(
                                        field.value.span,
                                        "`@asm` `volatile` flag must evaluate to a compile-time boolean constant",
                                    )
                                    .with_hint("example: `volatile: true` or `const VOL = true`")
                                    .emit();
                            }
                            Err(_) => {}
                        }
                    }
                }
                _ => {
                    self.ctx
                        .struct_error(
                            field.span,
                            format!("unknown field `{}` in `@asm` configuration", field_name),
                        )
                        .emit();
                    self.check_expr(&field.value, None);
                }
            }
        }

        if !has_asm {
            self.ctx
                .struct_error(
                    span,
                    "`@asm` configuration is missing the required `asm` template string",
                )
                .with_hint("example: `@asm(.{ asm: \"nop\", volatile: true })`")
                .emit();
        }

        self.ctx.node_types.insert(config_arg.id, TypeId::VOID);
        TypeId::VOID
    }

    fn is_mut_pointer(&mut self, ty: TypeId) -> bool {
        let norm = self.resolve_tv(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => is_mut,
            _ => false,
        }
    }
}
