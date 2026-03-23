use super::ExprChecker;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{BinaryOperator, Expr, UnaryOperator};
use kernc_utils::Span;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub fn check_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        // 1. 先检查左操作数，并解析它的真实类型
        let lhs_ty = self.check_expr(lhs, expected_ty);
        let l_norm = self.resolve_tv(lhs_ty);

        // 2. 提前判断左边是不是指针
        let is_l_ptr = matches!(
            self.ctx.type_registry.get(l_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );

        // 3. 计算右操作数的期望类型
        // 如果左边是指针，并且正在进行加减法，右边大概率是 usize/isize 偏移量（或另一个指针）。
        // 此时不能把左边的“指针类型”作为上下文硬塞给右边，否则右边的整型字面量（比如 + 1）会被错误地吸化成指针
        let rhs_expected = if is_l_ptr && (op == BinaryOperator::Add || op == BinaryOperator::Subtract) {
            None // 切断上下文感染，让右侧自然推导为整数
        } else {
            Some(lhs_ty) // 对于其他操作（如 Equal, ptr == 0），依然需要上下文让 0 化身为指针
        };

        // 4. 使用修复后的期望类型去检查右操作数
        let rhs_ty = self.check_expr(rhs, rhs_expected);
        let r_norm = self.resolve_tv(rhs_ty);

        // 5. 错误冒泡
        if l_norm == TypeId::ERROR || r_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        // 6. 判断右边是不是指针
        let is_r_ptr = matches!(
            self.ctx.type_registry.get(r_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );

        use BinaryOperator::*;
        match op {
            Add | Subtract => {
                if is_l_ptr || is_r_ptr {
                    if op == Add {
                        // ptr + int 或 int + ptr
                        if is_l_ptr && (r_norm == TypeId::USIZE || r_norm == TypeId::ISIZE) {
                            return l_norm;
                        }
                        if is_r_ptr && (l_norm == TypeId::USIZE || l_norm == TypeId::ISIZE) {
                            return r_norm;
                        }
                    } else if op == Subtract {
                        // ptr - int
                        if is_l_ptr && (r_norm == TypeId::USIZE || r_norm == TypeId::ISIZE) {
                            return l_norm;
                        }
                        // ptr - ptr (偏移量)
                        if is_l_ptr && is_r_ptr {
                            if l_norm == r_norm {
                                return TypeId::ISIZE; // 指针相减返回有符号整数
                            } else {
                                self.ctx.struct_error(lhs.span, "cannot subtract pointers of different types")
                                    .with_hint("both pointers must point to the exact same type")
                                    .emit();
                                return TypeId::ERROR;
                            }
                        }
                    }

                    // 拒绝非法的指针运算（如加上 i32 等隐式提升）
                    self.ctx.struct_error(lhs.span, "invalid pointer arithmetic")
                        .with_hint("pointer arithmetic requires `usize` or `isize` offsets, or subtraction between identical pointer types")
                        .emit();
                    return TypeId::ERROR;
                }

                // 常规数值加减法
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
            Multiply | Divide | Modulo => {
                if is_l_ptr || is_r_ptr {
                    self.ctx.struct_error(lhs.span, "multiplication, division, or modulo cannot be applied to pointers").emit();
                    return TypeId::ERROR;
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
            Equal | NotEqual | LessThan | GreaterThan | LessOrEqual | GreaterOrEqual => {
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                TypeId::BOOL
            }
            LogicalAnd | LogicalOr => {
                self.check_coercion(lhs.span, TypeId::BOOL, l_norm);
                self.check_coercion(rhs.span, TypeId::BOOL, r_norm);
                TypeId::BOOL
            }
            _ => {
                // Bitwise Ops
                if !self.ctx.type_registry.is_integer(l_norm) {
                    self.ctx
                        .struct_error(lhs.span, "bitwise operations require integer types")
                        .emit();
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
        }
    }

    pub fn check_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        span: Span,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let inner_expected = match op {
            UnaryOperator::Negate | UnaryOperator::BitwiseNot => expected_ty,
            // 兼容不可变取址和可变取址
            UnaryOperator::AddressOf | UnaryOperator::MutAddressOf => {
                if let Some(exp) = expected_ty {
                    let norm = self.resolve_tv(exp);
                    match self.ctx.type_registry.get(norm) {
                        TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                            Some(*elem)
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        let op_ty = self.check_expr(operand, inner_expected);
        if op_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        match op {
            UnaryOperator::AddressOf | UnaryOperator::MutAddressOf => {
                let is_mut = op == UnaryOperator::MutAddressOf;

                // 不允许对不可变的左值使用 `..&` 获取可变指针
                if is_mut && !self.is_lvalue_mutable(operand) {
                    self.ctx
                        .struct_error(
                            span,
                            "cannot take mutable address `..&` of immutable memory",
                        )
                        .with_hint(
                            "declare the variable with `let mut` or ensure the target is mutable",
                        )
                        .emit();
                }

                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: op_ty,
                })
            }
            UnaryOperator::PointerDeRef => {
                let norm = self.resolve_tv(op_ty);
                match self.ctx.type_registry.get(norm).clone() {
                    TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => elem,
                    _ => {
                        let ty_str = self.ctx.ty_to_string(op_ty);
                        self.ctx
                            .struct_error(span, "cannot dereference a non-pointer type")
                            .with_hint(format!("type is `{}`", ty_str))
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::LengthOf => {
                let norm = self.resolve_tv(op_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Array { .. } | TypeKind::Slice { .. } => TypeId::USIZE,
                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "length operator `#` can only be applied to arrays and slices",
                            )
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::Negate => {
                let op_ty_id = self.resolve_tv(op_ty);
                if !self.ctx.type_registry.is_integer(op_ty_id)
                    && !self.ctx.type_registry.is_float(op_ty_id)
                {
                    self.ctx
                        .struct_error(span, "negation requires a numeric type")
                        .emit();
                }
                op_ty
            }
            UnaryOperator::LogicalNot => {
                self.check_coercion(span, TypeId::BOOL, op_ty);
                TypeId::BOOL
            }
            UnaryOperator::BitwiseNot => {
                let op_ty_id = self.resolve_tv(op_ty);
                if !self.ctx.type_registry.is_integer(op_ty_id) {
                    self.ctx
                        .struct_error(span, "bitwise NOT requires an integer type")
                        .emit();
                }
                op_ty
            }
        }
    }

    pub fn check_assign(&mut self, lhs: &Expr, rhs: &Expr, span: Span) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        // 使用继承可变性分析器
        if !self.is_lvalue_mutable(lhs) && lhs_ty != TypeId::ERROR {
            self.ctx
                .struct_error(
                    lhs.span,
                    "cannot assign to an immutable variable or location",
                )
                .with_hint("if this is a variable, declare it with `let mut`")
                .with_hint(
                    "if this is a pointer dereference, ensure it is a mutable pointer (`*mut T`)",
                )
                .emit();
        }

        let l_norm = self.resolve_tv(lhs_ty);
        let rhs_ty = self.check_expr(rhs, Some(l_norm));

        if lhs_ty == TypeId::ERROR || rhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let rhs_ty_id = self.resolve_tv(rhs_ty);
        self.check_coercion(span, l_norm, rhs_ty_id);
        TypeId::VOID
    }
}
