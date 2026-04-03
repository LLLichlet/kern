use kernc_utils::Span;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token {
    ///  Token 的类型
    pub tag: TokenType,

    /// Token 所在的文件，以及它在文件中的具体位置
    pub span: Span,
}


#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenType {
    // === 标识符与字面量 ===
    
    /// 标识符。
    /// 
    /// 例如 `abc`，`my_var`。
    Identifier,

    /// 整型字面量。
    /// 
    /// 例如 `123`，`0xFF`。
    IntLiteral,

    /// 浮点字面量。
    /// 
    /// 例如 `3.14`。
    FloatLiteral,

    /// 字符串字面量。
    /// 
    /// "hello" or Zig-style \\ multiline
    StringLiteral,

    /// 字符字面量。
    /// 
    /// 例如 `'a'`。
    /// 
    /// 注意此处使用单引号（`'...'`）。
    CharLiteral,

    /// 字节字符字面量
    /// 
    /// 例如 `b'a'`。
    ByteCharLiteral,

    // === 关键字 ===
    
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

    // === 运算符 ===

    // 算术

    /// `+`
    Plus,

    /// `-`
    Minus,

    /// `*`
    /// 
    /// 同时用于指针解引用/类型 *T
    Star,

    /// `/`
    Slash,

    /// `%`
    Percent,

    // 特殊前缀

    /// #arr (长度)
    Hash,
    
    /// @intToFloat (内置函数)
    At,

    /// ^T (易失性指针) / Bitwise XOR 
    Caret,

    // 逻辑/位运算

    /// ! (宏调用 / 逻辑非)
    Bang,

    /// & (取地址 / 按位与)
    Ampersand,
    
    /// | (按位或)
    Pipe,

    /// ~ (按位取反)
    Tilde,

    // 比较

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

    // 移位

    /// `<<`
    LShift,

    /// `>>`
    RShift,

    // 赋值

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

    // === 标点 ===

    /// `.` (字段访问)
    Dot,

    /// `..` (Range)
    DotDot,

    /// `..=` (Range Inclusive)
    DotDotEqual,

    /// `.&` (不可变取地址)
    DotAmpersand,

    /// `.*` (指针解引用)
    DotStar,

    /// `.[` (不可变切片/数组索引)
    DotLBracket,

    /// `.{` (匿名结构体初始化)
    DotLBrace,

    /// `..&` (可变取地址)
    DotDotAmpersand,

    /// ``..[` (可变切片)
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

    // === 特殊 ===

    /// 解析错误
    Eof,
    
    /// 非法解析
    #[default]
    Illegal,

    /// 专门用于携带词法错误信息的 Token
    LexError(&'static str),
}
