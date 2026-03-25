use kernc_lexer::TokenType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,            // +
    Subtract,       // -
    Multiply,       // *
    Divide,         // /
    Modulo,         // %
    Equal,          // ==
    NotEqual,       // !=
    LessThan,       // <
    GreaterThan,    // >
    LessOrEqual,    // <=
    GreaterOrEqual, // >=
    LogicalAnd,     // and
    LogicalOr,      // or
    BitwiseAnd,     // &
    BitwiseOr,      // |
    BitwiseXor,     // ^
    ShiftLeft,      // <<
    ShiftRight,     // >>
}

impl BinaryOperator {
    pub fn from_token(token: TokenType) -> Self {
        match token {
            TokenType::Plus => Self::Add,
            TokenType::Minus => Self::Subtract,
            TokenType::Star => Self::Multiply,
            TokenType::Slash => Self::Divide,
            TokenType::Percent => Self::Modulo,
            TokenType::EqualEqual => Self::Equal,
            TokenType::NotEqual => Self::NotEqual,
            TokenType::LessThan => Self::LessThan,
            TokenType::GreaterThan => Self::GreaterThan,
            TokenType::LessEqual => Self::LessOrEqual,
            TokenType::GreaterEqual => Self::GreaterOrEqual,
            TokenType::And => Self::LogicalAnd,
            TokenType::Or => Self::LogicalOr,
            TokenType::Ampersand => Self::BitwiseAnd,
            TokenType::Pipe => Self::BitwiseOr,
            TokenType::Caret => Self::BitwiseXor,
            TokenType::LShift => Self::ShiftLeft,
            TokenType::RShift => Self::ShiftRight,
            _ => unreachable!("Token {:?} is not a binary operator", token),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Negate,       // -
    LogicalNot,   // !
    BitwiseNot,   // ~
    AddressOf,    // .&
    MutAddressOf, // ..&
    MetaOf,     // #
    PointerDeRef, // .*
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOperator {
    Assign,           // =
    AddAssign,        // +=
    SubtractAssign,   // -=
    MultiplyAssign,   // *=
    DivideAssign,     // /=
    ModuloAssign,     // %=
    BitwiseAndAssign, // &=
    BitwiseOrAssign,  // |=
    BitwiseXorAssign, // ^=
    ShiftLeftAssign,  // <<=
    ShiftRightAssign, // >>=
}

impl AssignmentOperator {
    pub fn from_token(token: TokenType) -> Self {
        match token {
            TokenType::Assign => Self::Assign,
            TokenType::PlusAssign => Self::AddAssign,
            TokenType::MinusAssign => Self::SubtractAssign,
            TokenType::StarAssign => Self::MultiplyAssign,
            TokenType::SlashAssign => Self::DivideAssign,
            TokenType::PercentAssign => Self::ModuloAssign,
            TokenType::AmpersandAssign => Self::BitwiseAndAssign,
            TokenType::PipeAssign => Self::BitwiseOrAssign,
            TokenType::CaretAssign => Self::BitwiseXorAssign,
            TokenType::LShiftAssign => Self::ShiftLeftAssign,
            TokenType::RShiftAssign => Self::ShiftRightAssign,
            _ => unreachable!("Token {:?} is not an assignment operator", token),
        }
    }
}
