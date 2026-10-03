//! The expression arena: symbolic expression trees the core holds for
//! `<math>`. A math cell is an ordinary `f64` cell whose value is an index
//! into this arena. Expressions are built once at build time; ticks never
//! create or rewrite them, they only evaluate (see `Op::Evaluate` and
//! `Op::EvalAt`). A real expression library would replace the operations
//! here; the arena's shape (indices into a flat node list, numeric leaves
//! bound to cells) is the interface the cell architecture needs from it.

use crate::document::CellIdx;

pub type ExprId = u32;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    /// A free symbol such as `x`.
    Sym(String),
    /// A numeric leaf bound to a cell: `$a` inside a math.
    Cell(CellIdx),
    Add(ExprId, ExprId),
    Sub(ExprId, ExprId),
    Mul(ExprId, ExprId),
    Div(ExprId, ExprId),
    Pow(ExprId, ExprId),
    Neg(ExprId),
}

#[derive(Debug, Clone, Default)]
pub struct Arena {
    pub nodes: Vec<Expr>,
}

impl Arena {
    pub fn push(&mut self, e: Expr) -> ExprId {
        // Fold constants as they are built.
        let folded = match &e {
            Expr::Add(a, b) => self.fold2(*a, *b, |x, y| x + y),
            Expr::Sub(a, b) => self.fold2(*a, *b, |x, y| x - y),
            Expr::Mul(a, b) => self.fold2(*a, *b, |x, y| x * y),
            Expr::Div(a, b) => self.fold2(*a, *b, |x, y| x / y),
            Expr::Pow(a, b) => self.fold2(*a, *b, f64::powf),
            Expr::Neg(a) => match self.nodes[*a as usize] {
                Expr::Num(x) => Some(Expr::Num(-x)),
                _ => None,
            },
            _ => None,
        };
        self.nodes.push(folded.unwrap_or(e));
        (self.nodes.len() - 1) as ExprId
    }

    fn fold2(&self, a: ExprId, b: ExprId, f: impl Fn(f64, f64) -> f64) -> Option<Expr> {
        match (&self.nodes[a as usize], &self.nodes[b as usize]) {
            (Expr::Num(x), Expr::Num(y)) => Some(Expr::Num(f(*x, *y))),
            _ => None,
        }
    }

    pub fn get(&self, id: ExprId) -> &Expr {
        &self.nodes[id as usize]
    }

    /// Whether the expression has no free symbols, so it can be lowered to
    /// numeric operators at build time.
    pub fn is_numeric(&self, id: ExprId) -> bool {
        match self.get(id) {
            Expr::Num(_) | Expr::Cell(_) => true,
            Expr::Sym(_) => false,
            Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) | Expr::Div(a, b) | Expr::Pow(a, b) => self.is_numeric(*a) && self.is_numeric(*b),
            Expr::Neg(a) => self.is_numeric(*a),
        }
    }

    /// The free symbols, in first-appearance order.
    pub fn symbols(&self, id: ExprId, out: &mut Vec<String>) {
        match self.get(id) {
            Expr::Sym(s) => {
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
            Expr::Num(_) | Expr::Cell(_) => {}
            Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) | Expr::Div(a, b) | Expr::Pow(a, b) => {
                self.symbols(*a, out);
                self.symbols(*b, out);
            }
            Expr::Neg(a) => self.symbols(*a, out),
        }
    }

    /// Evaluate to a number, reading cell leaves from `cells`. A free symbol
    /// evaluates to NaN unless `subst` gives it a value (every symbol gets
    /// the same value: the single-variable "evaluate at" of the experiment).
    pub fn eval(&self, id: ExprId, cells: &[f64], subst: Option<f64>) -> f64 {
        match self.get(id) {
            Expr::Num(v) => *v,
            Expr::Sym(_) => subst.unwrap_or(f64::NAN),
            Expr::Cell(c) => cells[*c as usize],
            Expr::Add(a, b) => self.eval(*a, cells, subst) + self.eval(*b, cells, subst),
            Expr::Sub(a, b) => self.eval(*a, cells, subst) - self.eval(*b, cells, subst),
            Expr::Mul(a, b) => self.eval(*a, cells, subst) * self.eval(*b, cells, subst),
            Expr::Div(a, b) => self.eval(*a, cells, subst) / self.eval(*b, cells, subst),
            Expr::Pow(a, b) => self.eval(*a, cells, subst).powf(self.eval(*b, cells, subst)),
            Expr::Neg(a) => -self.eval(*a, cells, subst),
        }
    }

    /// Distinct cell leaves, in first-appearance order.
    pub fn cell_leaves(&self, id: ExprId, out: &mut Vec<CellIdx>) {
        match self.get(id) {
            Expr::Cell(c) => {
                if !out.contains(c) {
                    out.push(*c);
                }
            }
            Expr::Num(_) | Expr::Sym(_) => {}
            Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) | Expr::Div(a, b) | Expr::Pow(a, b) => {
                self.cell_leaves(*a, out);
                self.cell_leaves(*b, out);
            }
            Expr::Neg(a) => self.cell_leaves(*a, out),
        }
    }

    /// Rebind cell leaves (slots during build become cells afterwards).
    pub fn map_cells(&mut self, f: impl Fn(CellIdx) -> CellIdx) {
        for n in &mut self.nodes {
            if let Expr::Cell(c) = n {
                *c = f(*c);
            }
        }
    }

    /// Infix text, with cell leaves shown by `cell_name`.
    pub fn display(&self, id: ExprId, cell_name: &dyn Fn(CellIdx) -> String) -> String {
        fn prec(e: &Expr) -> u8 {
            match e {
                Expr::Add(..) | Expr::Sub(..) => 1,
                Expr::Mul(..) | Expr::Div(..) => 2,
                Expr::Neg(..) => 3,
                Expr::Pow(..) => 4,
                _ => 5,
            }
        }
        fn go(a: &Arena, id: ExprId, min: u8, name: &dyn Fn(CellIdx) -> String) -> String {
            let e = a.get(id);
            let p = prec(e);
            let s = match e {
                Expr::Num(v) => format!("{v}"),
                Expr::Sym(s) => s.clone(),
                Expr::Cell(c) => name(*c),
                Expr::Add(x, y) => format!("{} + {}", go(a, *x, 1, name), go(a, *y, 2, name)),
                Expr::Sub(x, y) => format!("{} - {}", go(a, *x, 1, name), go(a, *y, 2, name)),
                Expr::Mul(x, y) => format!("{} * {}", go(a, *x, 2, name), go(a, *y, 3, name)),
                Expr::Div(x, y) => format!("{} / {}", go(a, *x, 2, name), go(a, *y, 3, name)),
                Expr::Pow(x, y) => format!("{}^{}", go(a, *x, 5, name), go(a, *y, 4, name)),
                Expr::Neg(x) => format!("-{}", go(a, *x, 3, name)),
            };
            if p < min { format!("({s})") } else { s }
        }
        go(self, id, 0, cell_name)
    }

    pub fn heap_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Expr>()
    }
}

/// A token of math text. References (`$a`) arrive already resolved to a
/// cell (or build slot) by the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Num(f64),
    Ident(String),
    Cell(CellIdx),
    Op(char),
    LParen,
    RParen,
}

/// Tokenize one run of text. Identifiers are maximal letter runs (`pi` is a
/// constant, `x`, `ab` are symbols); numbers are decimal literals.
pub fn tokenize(text: &str, out: &mut Vec<Token>) -> Result<(), String> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == '-' || *d == '+') {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let s: String = chars[start..i].iter().collect();
            out.push(Token::Num(s.parse().map_err(|_| format!("bad number '{s}'"))?));
        } else if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphanumeric() {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            out.push(match s.as_str() {
                "pi" => Token::Num(std::f64::consts::PI),
                _ => Token::Ident(s),
            });
        } else {
            out.push(match c {
                '+' | '-' | '*' | '/' | '^' => Token::Op(c),
                '(' => Token::LParen,
                ')' => Token::RParen,
                other => return Err(format!("unexpected '{other}'")),
            });
            i += 1;
        }
    }
    Ok(())
}

/// Recursive-descent parser over tokens: `+ - * / ^`, unary minus,
/// parentheses, and juxtaposition as multiplication (`3x`, `2(x+1)`).
pub struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    arena: &'a mut Arena,
}

impl<'a> Parser<'a> {
    pub fn parse(toks: &'a [Token], arena: &'a mut Arena) -> Result<ExprId, String> {
        let mut p = Parser { toks, pos: 0, arena };
        let id = p.expr()?;
        if p.pos != p.toks.len() {
            return Err(format!("unexpected token {:?}", p.toks[p.pos]));
        }
        Ok(id)
    }

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn expr(&mut self) -> Result<ExprId, String> {
        let mut lhs = self.term()?;
        while let Some(Token::Op(c @ ('+' | '-'))) = self.peek() {
            let c = *c;
            self.pos += 1;
            let rhs = self.term()?;
            lhs = self.arena.push(if c == '+' { Expr::Add(lhs, rhs) } else { Expr::Sub(lhs, rhs) });
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<ExprId, String> {
        let mut lhs = self.unary()?;
        loop {
            match self.peek() {
                Some(Token::Op(c @ ('*' | '/'))) => {
                    let c = *c;
                    self.pos += 1;
                    let rhs = self.unary()?;
                    lhs = self.arena.push(if c == '*' { Expr::Mul(lhs, rhs) } else { Expr::Div(lhs, rhs) });
                }
                // Juxtaposition: a factor directly followed by another.
                Some(Token::Num(_) | Token::Ident(_) | Token::Cell(_) | Token::LParen) => {
                    let rhs = self.power()?;
                    lhs = self.arena.push(Expr::Mul(lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn unary(&mut self) -> Result<ExprId, String> {
        if let Some(Token::Op('-')) = self.peek() {
            self.pos += 1;
            let e = self.unary()?;
            return Ok(self.arena.push(Expr::Neg(e)));
        }
        if let Some(Token::Op('+')) = self.peek() {
            self.pos += 1;
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Result<ExprId, String> {
        let base = self.atom()?;
        if let Some(Token::Op('^')) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?;
            return Ok(self.arena.push(Expr::Pow(base, exp)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<ExprId, String> {
        let t = self.peek().cloned().ok_or_else(|| "unexpected end of expression".to_string())?;
        self.pos += 1;
        Ok(match t {
            Token::Num(v) => self.arena.push(Expr::Num(v)),
            Token::Ident(s) => self.arena.push(Expr::Sym(s)),
            Token::Cell(c) => self.arena.push(Expr::Cell(c)),
            Token::LParen => {
                let e = self.expr()?;
                match self.peek() {
                    Some(Token::RParen) => self.pos += 1,
                    _ => return Err("missing ')'".into()),
                }
                e
            }
            other => return Err(format!("unexpected token {other:?}")),
        })
    }
}
