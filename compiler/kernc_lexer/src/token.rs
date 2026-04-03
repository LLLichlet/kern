use kernc_utils::Span;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token {
    /// Token kind.
    pub tag: TokenType,

    /// File-relative source span for this token.
    pub span: Span,
}


#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenType {
    // === Identifiers and literals ===
    
    /// Identifier token, for example `abc` or `my_var`.
    Identifier,

    /// Integer literal, for example `123` or `0xFF`.
    IntLiteral,

    /// Floating-point literal, for example `3.14`.
    FloatLiteral,

    /// String literal, including Zig-style `\\` multiline strings.
    StringLiteral,

    /// Character literal delimited by single quotes, for example `'a'`.
    CharLiteral,

    /// Byte character literal, for example `b'a'`.
    ByteCharLiteral,

    /// Outer doc comment, for example `/// Summary`.
    DocCommentOuter,

    /// Inner doc comment, for example `//! Module summary`.
    DocCommentInner,

    // === Keywords ===
    
    Fn, 
    Let,
    Mut,
    Const,
    Static,
    Type,
    Struct,
    Union,
    Enum,
    Trait,
    If,
    Else,
    For,
    Break,
    Continue,
    Return,
    Defer,
    Pub,
    Extern,
    Use,
    Impl,
    True,
    False,
    Undef,
    As,
    And,
    Or,
    Underscore,
    SelfType,
    SelfValue,
    Match,
    Mod,
    Where,
    CapitalFn,
    Void,

    // === Operators ===

    // Arithmetic

    /// `+`
    Plus,

    /// `-`
    Minus,

    /// `*`
    /// 
    /// Also used for pointer dereference and pointer types such as `*T`.
    Star,

    /// `/`
    Slash,

    /// `%`
    Percent,

    // Special prefixes

    /// `#arr` length operator.
    Hash,
    
    /// `@intToFloat`-style intrinsic prefix.
    At,

    /// Volatile pointer syntax `^T` or bitwise XOR.
    Caret,

    // Logical and bitwise operators

    /// `!` for macro-style calls or logical negation.
    Bang,

    /// `&` for address-of or bitwise AND.
    Ampersand,
    
    /// `|` bitwise OR.
    Pipe,

    /// `~` bitwise NOT.
    Tilde,

    // Comparisons

    /// `==`
    EqualEqual,

    /// `!=`
    NotEqual,
    
    /// `<`
    LessThan,
    
    /// `<=`
    LessEqual,
    
    /// `>`
    GreaterThan,
    
    /// `>=`
    GreaterEqual,

    // Shifts

    /// `<<`
    LShift,

    /// `>>`
    RShift,

    // Assignments

    /// `=`
    Assign,

    /// `+=`
    PlusAssign,

    /// `-=`
    MinusAssign,

    /// `*=`
    StarAssign,

    /// `/=`
    SlashAssign,

    /// `%=`
    PercentAssign,

    /// `&=`
    AmpersandAssign,

    /// `|=`
    PipeAssign,
    
    /// `^=`
    CaretAssign,

    /// `<<=`
    LShiftAssign,
    
    /// `>>=`
    RShiftAssign,

    // === Punctuation ===

    /// `.` field access.
    Dot,

    /// `..` range.
    DotDot,

    /// `..=` inclusive range.
    DotDotEqual,

    /// `.&` immutable address-of.
    DotAmpersand,

    /// `.*` pointer dereference.
    DotStar,

    /// `.[` immutable slice or array indexing.
    DotLBracket,

    /// `.{` anonymous aggregate construction.
    DotLBrace,

    /// `..&` mutable address-of.
    DotDotAmpersand,

    /// `..[` mutable slicing.
    DotDotLBracket,

    /// ...
    Ellipsis,

    /// ,
    Comma,

    /// :
    Colon,

    /// ;
    Semicolon,

    /// (
    LParen,

    /// )
    RParen,

    /// {
    LBrace,
    
    /// }
    RBrace,

    /// [
    LBracket,
    
    /// ]
    RBracket,

    /// =>
    Arrow,

    // === Special ===

    /// End of file sentinel.
    Eof,
    
    /// Invalid token produced after recovery.
    #[default]
    Illegal,

    /// Token carrying a lexer error message.
    LexError(&'static str),
}
