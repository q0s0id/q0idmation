//! The shape of a q0ex program. The block editor works on these nodes
//! directly, and [`crate::printer`] turns them back into text.

use std::rc::Rc;

/// A whole plugin file, top-level items in their written order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub line: u32,
    /// Where the block editor shows it (`#@ x y` above it in the text).
    pub pos: Option<(f32, f32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// `plugin "Name" { key = value }`: the card in the plugin list.
    Plugin {
        name: String,
        fields: Vec<(String, Expr)>,
    },
    /// `settings { ... }`: what the plugin's settings page shows.
    Settings(Vec<SettingDecl>),
    /// `page ~~~ ... ~~~`: the plugin's page, in its own markup.
    Page(String),
    /// `blob name kind ~~~ base64 ~~~`: a picture or other file.
    Blob {
        name: String,
        kind: String,
        data: String,
    },
    /// `when event "arg" params { }`
    When(When),
    /// `fn name(params) { }`
    Fn(FnDef),
    /// `keep name = value`: a variable that outlives the editor.
    Keep {
        name: String,
        value: Expr,
    },
    Stmt(Stmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct When {
    pub event: String,
    /// The quoted word after the event: the menu label, window title, key.
    pub arg: Option<String>,
    pub params: Vec<String>,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Block,
}

pub type Block = Rc<Vec<Stmt>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
}

impl AssignOp {
    pub fn text(self) -> &'static str {
        match self {
            Self::Set => "=",
            Self::Add => "+=",
            Self::Sub => "-=",
            Self::Mul => "*=",
            Self::Div => "/=",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Expr(Expr),
    Assign {
        target: Expr,
        op: AssignOp,
        value: Expr,
    },
    /// `a <-> b`
    Swap(Expr, Expr),
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    /// `repeat n { }`
    Repeat {
        count: Expr,
        body: Block,
    },
    /// `for item in list { }`, `for key, value in map { }`
    For {
        var: String,
        second: Option<String>,
        iter: Expr,
        body: Block,
    },
    Break,
    Continue,
    Return(Option<Expr>),
    /// `try { } oops err { }`
    Try {
        body: Block,
        name: Option<String>,
        handler: Block,
    },
    /// `do! { }`: runs right away, in the same scope.
    Do(Block),
    /// `later! 2s { }`
    Later {
        delay: Expr,
        body: Block,
    },
    /// `every! 100ms { }`; `break` inside stops it.
    Every {
        interval: Expr,
        body: Block,
    },
    /// A function defined inside another block.
    Fn(FnDef),
    Comment(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: u32,
}

impl Expr {
    pub fn new(kind: ExprKind, line: u32) -> Self {
        Self { kind, line }
    }

    pub fn null(line: u32) -> Self {
        Self::new(ExprKind::Null, line)
    }
}

/// The unit written after a number. Only the text changes; `500ms` is
/// half a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Ms,
    S,
    Deg,
    Px,
}

impl Unit {
    pub fn text(self) -> &'static str {
        match self {
            Self::Ms => "ms",
            Self::S => "s",
            Self::Deg => "deg",
            Self::Px => "px",
        }
    }

    pub fn scale(self) -> f64 {
        match self {
            Self::Ms => 0.001,
            _ => 1.0,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "ms" => Self::Ms,
            "s" => Self::S,
            "deg" => Self::Deg,
            "px" => Self::Px,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
            Self::Pow => "^",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
        }
    }

    /// Binding strength; higher binds tighter.
    pub fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Eq | Self::Ne => 3,
            Self::Lt | Self::Le | Self::Gt | Self::Ge => 4,
            Self::Add | Self::Sub => 6,
            Self::Mul | Self::Div | Self::Rem => 7,
            Self::Pow => 8,
        }
    }
}

/// A piece of a backtick string: text, or an expression in `{}`.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePart {
    Text(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Num(f64, Option<Unit>),
    Str(String),
    Template(Vec<TemplatePart>),
    Var(String),
    List(Vec<Expr>),
    Map(Vec<(String, Expr)>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// `f(a, b)`, or `f! a, b` when `bang` (written without parentheses).
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        bang: bool,
    },
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `a..b` (both ends) or `a..<b` (without the end).
    Range {
        from: Box<Expr>,
        to: Box<Expr>,
        inclusive: bool,
    },
    /// `value |> f(b)` is `f(value, b)`.
    Pipe(Box<Expr>, Box<Expr>),
    Lambda {
        params: Vec<String>,
        body: Block,
    },
}

/// One line of `settings { }`.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingDecl {
    pub kind: SettingKind,
    /// The name code reads it by: `config.size`. Empty for headings and notes.
    pub key: String,
    pub label: String,
    pub default: Option<Expr>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    /// `choice`: the values to pick from.
    pub options: Vec<Expr>,
    /// `? "..."`: shown when hovering it.
    pub help: Option<String>,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Slider,
    Number,
    Toggle,
    Text,
    Color,
    Choice,
    Button,
    Heading,
    Note,
    /// A key or chord the person picks, like `ctrl+k`: `when key "@name"`
    /// runs when it is pressed.
    Key,
}

impl SettingKind {
    pub const ALL: [Self; 10] = [
        Self::Slider,
        Self::Number,
        Self::Toggle,
        Self::Text,
        Self::Color,
        Self::Choice,
        Self::Button,
        Self::Heading,
        Self::Note,
        Self::Key,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Self::Slider => "slider",
            Self::Number => "number",
            Self::Toggle => "toggle",
            Self::Text => "text",
            Self::Color => "color",
            Self::Choice => "choice",
            Self::Button => "button",
            Self::Heading => "heading",
            Self::Note => "note",
            Self::Key => "key",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// Headings and notes only shape the page; they hold no value.
    pub fn has_key(self) -> bool {
        !matches!(self, Self::Heading | Self::Note)
    }
}
