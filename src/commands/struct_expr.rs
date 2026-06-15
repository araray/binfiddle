//! Expression evaluation for structural templates.
//!
//! Provides a small, safe expression language used by the `struct` command
//! to support dynamic offsets, sizes, conditional fields, computed values,
//! and bitfield references.
//!
//! # Supported Syntax
//!
//! - Numbers: `42`, `0x2a`
//! - Variables: `$fieldname`, `$@current`, `$@prev_end`, `$@file_size`, `$@base`
//! - Bitfield references: `$flags.is_compressed`
//! - Arithmetic: `+`, `-`, `*`, `/`, `%`, `**`
//! - Comparisons: `==`, `!=`, `<`, `<=`, `>`, `>=`
//! - Logic: `and`/`&&`, `or`/`||`, `not`
//! - Parentheses for grouping

use crate::error::{BinfiddleError, Result};
use std::collections::HashMap;

/// Context available during expression evaluation.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    /// Named field values (including bitfield values as `fieldname.bitfield`).
    variables: HashMap<String, i128>,
    /// Current parse offset.
    current_offset: i128,
    /// End offset of the previously parsed field.
    prev_end: i128,
    /// Total size of the input data.
    file_size: i128,
    /// Base offset of the current template (for nested templates).
    base_offset: i128,
}

impl EvalContext {
    /// Create a new context with magic variables initialized.
    pub fn new(file_size: usize, base_offset: usize) -> Self {
        Self {
            variables: HashMap::new(),
            current_offset: base_offset as i128,
            prev_end: base_offset as i128,
            file_size: file_size as i128,
            base_offset: base_offset as i128,
        }
    }

    /// Set the current parse offset.
    pub fn set_current_offset(&mut self, offset: usize) {
        self.current_offset = offset as i128;
    }

    /// Set the end offset of the previous parsed field.
    pub fn set_prev_end(&mut self, end: usize) {
        self.prev_end = end as i128;
    }

    /// Get the end offset of the previous parsed field.
    pub fn prev_end(&self) -> usize {
        self.prev_end as usize
    }

    /// Insert a named variable value.
    pub fn set_variable(&mut self, name: impl Into<String>, value: i128) {
        self.variables.insert(name.into(), value);
    }

    /// Create a child context for parsing at a new base offset, copying all variables.
    pub fn child_with_base(&self, base_offset: usize) -> Self {
        Self {
            variables: self.variables.clone(),
            current_offset: base_offset as i128,
            prev_end: base_offset as i128,
            file_size: self.file_size,
            base_offset: base_offset as i128,
        }
    }

    /// Get a variable value.
    fn get(&self, name: &str) -> Option<i128> {
        match name {
            "@current" => Some(self.current_offset),
            "@prev_end" => Some(self.prev_end),
            "@file_size" => Some(self.file_size),
            "@base" => Some(self.base_offset),
            _ => self.variables.get(name).copied(),
        }
    }
}

/// Token produced by the expression tokenizer.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(i128),
    Variable(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Power,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    LParen,
    RParen,
    Eof,
}

/// Tokenizes an expression string.
fn tokenize(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if c.is_whitespace() {
            i += 1;
            continue;
        }

        if c == '$' {
            // Variable: $@name or $name or $name.subname
            i += 1;
            let mut name = String::new();
            if i < chars.len() && chars[i] == '@' {
                name.push('@');
                i += 1;
            }
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            name.push_str(&input[start..i]);
            if name.is_empty() || name == "@" {
                return Err(BinfiddleError::Parse(format!(
                    "Empty variable reference in expression '{}'",
                    input
                )));
            }
            tokens.push(Token::Variable(name));
            continue;
        }

        if c.is_ascii_digit() || (c == '0' && i + 1 < chars.len() && chars[i + 1] == 'x') {
            let start = i;
            if c == '0' && i + 1 < chars.len() && chars[i + 1] == 'x' {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let num_str = &input[start + 2..i];
                let value = i128::from_str_radix(num_str, 16).map_err(|e| {
                    BinfiddleError::Parse(format!(
                        "Invalid hex number '{}' in expression '{}': {}",
                        num_str, input, e
                    ))
                })?;
                tokens.push(Token::Number(value));
            } else {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let num_str = &input[start..i];
                let value = num_str.parse::<i128>().map_err(|e| {
                    BinfiddleError::Parse(format!(
                        "Invalid number '{}' in expression '{}': {}",
                        num_str, input, e
                    ))
                })?;
                tokens.push(Token::Number(value));
            }
            continue;
        }

        if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            let word = &input[start..i];
            let token = match word.to_lowercase().as_str() {
                "and" => Token::And,
                "or" => Token::Or,
                "not" => Token::Not,
                _ => {
                    return Err(BinfiddleError::Parse(format!(
                        "Unknown keyword '{}' in expression '{}'",
                        word, input
                    )))
                }
            };
            tokens.push(token);
            continue;
        }

        let (token, advance) = match c {
            '+' => (Token::Plus, 1),
            '-' => (Token::Minus, 1),
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    (Token::Power, 2)
                } else {
                    (Token::Star, 1)
                }
            }
            '/' => (Token::Slash, 1),
            '%' => (Token::Percent, 1),
            '(' => (Token::LParen, 1),
            ')' => (Token::RParen, 1),
            '<' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    (Token::Le, 2)
                } else {
                    (Token::Lt, 1)
                }
            }
            '>' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    (Token::Ge, 2)
                } else {
                    (Token::Gt, 1)
                }
            }
            '=' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    (Token::Eq, 2)
                } else {
                    return Err(BinfiddleError::Parse(format!(
                        "Unexpected '=' in expression '{}' (did you mean '=='?)",
                        input
                    )));
                }
            }
            '!' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    (Token::Ne, 2)
                } else {
                    return Err(BinfiddleError::Parse(format!(
                        "Unexpected '!' in expression '{}' (did you mean '!='?)",
                        input
                    )));
                }
            }
            '&' => {
                if i + 1 < chars.len() && chars[i + 1] == '&' {
                    (Token::And, 2)
                } else {
                    return Err(BinfiddleError::Parse(format!(
                        "Unexpected '&' in expression '{}' (did you mean '&&'?)",
                        input
                    )));
                }
            }
            '|' => {
                if i + 1 < chars.len() && chars[i + 1] == '|' {
                    (Token::Or, 2)
                } else {
                    return Err(BinfiddleError::Parse(format!(
                        "Unexpected '|' in expression '{}' (did you mean '||'?)",
                        input
                    )));
                }
            }
            _ => {
                return Err(BinfiddleError::Parse(format!(
                    "Unexpected character '{}' in expression '{}'",
                    c, input
                )))
            }
        };

        tokens.push(token);
        i += advance;
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

/// Type alias for binary expression constructors.
type BinExprCtor = fn(Box<Expr>, Box<Expr>) -> Expr;

/// Parsed expression tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(i128),
    Variable(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Mod(Box<Expr>, Box<Expr>),
    Pow(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

/// Recursive descent parser for expressions.
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn next(&mut self) -> Token {
        let token = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn expect(&mut self, expected: Token) -> Result<()> {
        let token = self.next();
        if std::mem::discriminant(&token) != std::mem::discriminant(&expected) {
            return Err(BinfiddleError::Parse(format!(
                "Expected {:?}, got {:?}",
                expected, token
            )));
        }
        Ok(())
    }

    fn parse(&mut self) -> Result<Expr> {
        let expr = self.parse_or()?;
        self.expect(Token::Eof)?;
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Token::Or) {
            self.next();
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_not()?;
        while matches!(self.peek(), Token::And) {
            self.next();
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr> {
        if matches!(self.peek(), Token::Not) {
            self.next();
            let expr = self.parse_not()?;
            Ok(Expr::Not(Box::new(expr)))
        } else {
            self.parse_comparison()
        }
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let left = self.parse_additive()?;
        let op: Option<BinExprCtor> = match self.peek() {
            Token::Eq => Some(Expr::Eq as BinExprCtor),
            Token::Ne => Some(Expr::Ne as BinExprCtor),
            Token::Lt => Some(Expr::Lt as BinExprCtor),
            Token::Le => Some(Expr::Le as BinExprCtor),
            Token::Gt => Some(Expr::Gt as BinExprCtor),
            Token::Ge => Some(Expr::Ge as BinExprCtor),
            _ => None,
        };

        if let Some(ctor) = op {
            self.next();
            let right = self.parse_additive()?;
            Ok(ctor(Box::new(left), Box::new(right)))
        } else {
            Ok(left)
        }
    }

    fn parse_additive(&mut self) -> Result<Expr> {
        let mut left = self.parse_multiplicative()?;
        loop {
            match self.peek() {
                Token::Plus => {
                    self.next();
                    let right = self.parse_multiplicative()?;
                    left = Expr::Add(Box::new(left), Box::new(right));
                }
                Token::Minus => {
                    self.next();
                    let right = self.parse_multiplicative()?;
                    left = Expr::Sub(Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr> {
        let mut left = self.parse_power()?;
        loop {
            match self.peek() {
                Token::Star => {
                    self.next();
                    let right = self.parse_power()?;
                    left = Expr::Mul(Box::new(left), Box::new(right));
                }
                Token::Slash => {
                    self.next();
                    let right = self.parse_power()?;
                    left = Expr::Div(Box::new(left), Box::new(right));
                }
                Token::Percent => {
                    self.next();
                    let right = self.parse_power()?;
                    left = Expr::Mod(Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_power(&mut self) -> Result<Expr> {
        let base = self.parse_unary()?;
        if matches!(self.peek(), Token::Power) {
            self.next();
            let exp = self.parse_unary()?;
            Ok(Expr::Pow(Box::new(base), Box::new(exp)))
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        match self.peek() {
            Token::Minus => {
                self.next();
                let expr = self.parse_unary()?;
                Ok(Expr::Neg(Box::new(expr)))
            }
            Token::Plus => {
                self.next();
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.next() {
            Token::Number(n) => Ok(Expr::Number(n)),
            Token::Variable(name) => Ok(Expr::Variable(name)),
            Token::LParen => {
                let expr = self.parse_or()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            token => Err(BinfiddleError::Parse(format!(
                "Unexpected token {:?} in expression",
                token
            ))),
        }
    }
}

/// Parses an expression string into an AST.
pub fn parse_expression(input: &str) -> Result<Expr> {
    let tokens = tokenize(input)?;
    let mut parser = Parser::new(tokens);
    parser.parse()
}

/// Evaluates an expression to an integer value.
pub fn evaluate(expr: &Expr, ctx: &EvalContext) -> Result<i128> {
    match expr {
        Expr::Number(n) => Ok(*n),
        Expr::Variable(name) => ctx.get(name).ok_or_else(|| {
            BinfiddleError::Parse(format!(
                "Unknown variable '${}' in expression (available: current, prev_end, file_size, base, and parsed fields)",
                name
            ))
        }),
        Expr::Neg(e) => Ok(-evaluate(e, ctx)?),
        Expr::Not(e) => {
            let v = evaluate(e, ctx)?;
            Ok(if v == 0 { 1 } else { 0 })
        }
        Expr::Add(a, b) => Ok(evaluate(a, ctx)? + evaluate(b, ctx)?),
        Expr::Sub(a, b) => Ok(evaluate(a, ctx)? - evaluate(b, ctx)?),
        Expr::Mul(a, b) => Ok(evaluate(a, ctx)? * evaluate(b, ctx)?),
        Expr::Div(a, b) => {
            let denom = evaluate(b, ctx)?;
            if denom == 0 {
                return Err(BinfiddleError::Parse(
                    "Division by zero in expression".to_string(),
                ));
            }
            Ok(evaluate(a, ctx)? / denom)
        }
        Expr::Mod(a, b) => {
            let denom = evaluate(b, ctx)?;
            if denom == 0 {
                return Err(BinfiddleError::Parse(
                    "Modulo by zero in expression".to_string(),
                ));
            }
            Ok(evaluate(a, ctx)? % denom)
        }
        Expr::Pow(a, b) => {
            let base = evaluate(a, ctx)?;
            let exp = evaluate(b, ctx)?;
            if exp < 0 {
                return Err(BinfiddleError::Parse(
                    "Negative exponent in expression".to_string(),
                ));
            }
            Ok(base.pow(exp as u32))
        }
        Expr::Eq(a, b) => {
            let left = evaluate(a, ctx)?;
            let right = evaluate(b, ctx)?;
            Ok(if left == right { 1 } else { 0 })
        }
        Expr::Ne(a, b) => {
            let left = evaluate(a, ctx)?;
            let right = evaluate(b, ctx)?;
            Ok(if left != right { 1 } else { 0 })
        }
        Expr::Lt(a, b) => {
            let left = evaluate(a, ctx)?;
            let right = evaluate(b, ctx)?;
            Ok(if left < right { 1 } else { 0 })
        }
        Expr::Le(a, b) => {
            let left = evaluate(a, ctx)?;
            let right = evaluate(b, ctx)?;
            Ok(if left <= right { 1 } else { 0 })
        }
        Expr::Gt(a, b) => {
            let left = evaluate(a, ctx)?;
            let right = evaluate(b, ctx)?;
            Ok(if left > right { 1 } else { 0 })
        }
        Expr::Ge(a, b) => {
            let left = evaluate(a, ctx)?;
            let right = evaluate(b, ctx)?;
            Ok(if left >= right { 1 } else { 0 })
        }
        Expr::And(a, b) => {
            let left = evaluate(a, ctx)?;
            if left == 0 {
                Ok(0)
            } else {
                let right = evaluate(b, ctx)?;
                Ok(if right != 0 { 1 } else { 0 })
            }
        }
        Expr::Or(a, b) => {
            let left = evaluate(a, ctx)?;
            if left != 0 {
                Ok(1)
            } else {
                let right = evaluate(b, ctx)?;
                Ok(if right != 0 { 1 } else { 0 })
            }
        }
    }
}

/// Evaluates an expression string directly to an integer.
pub fn eval_to_i128(input: &str, ctx: &EvalContext) -> Result<i128> {
    let expr = parse_expression(input)?;
    evaluate(&expr, ctx)
}

/// Evaluates an expression string as a boolean.
pub fn eval_to_bool(input: &str, ctx: &EvalContext) -> Result<bool> {
    Ok(eval_to_i128(input, ctx)? != 0)
}

/// Resolves a value-or-expression string to a non-negative size/offset.
pub fn resolve_size_or_offset(input: &str, ctx: &EvalContext) -> Result<usize> {
    let value = eval_to_i128(input, ctx)?;
    if value < 0 {
        return Err(BinfiddleError::InvalidRange(format!(
            "Expression '{}' evaluated to negative value {}",
            input, value
        )));
    }
    value
        .try_into()
        .map_err(|_| BinfiddleError::InvalidRange(format!("Value {} exceeds usize range", value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_numbers() {
        let ctx = EvalContext::new(100, 0);
        assert_eq!(eval_to_i128("42", &ctx).unwrap(), 42);
        assert_eq!(eval_to_i128("0x2a", &ctx).unwrap(), 42);
    }

    #[test]
    fn test_arithmetic() {
        let ctx = EvalContext::new(100, 0);
        assert_eq!(eval_to_i128("2 + 3 * 4", &ctx).unwrap(), 14);
        assert_eq!(eval_to_i128("(2 + 3) * 4", &ctx).unwrap(), 20);
        assert_eq!(eval_to_i128("10 - 3", &ctx).unwrap(), 7);
        assert_eq!(eval_to_i128("2 ** 8", &ctx).unwrap(), 256);
        assert_eq!(eval_to_i128("17 % 5", &ctx).unwrap(), 2);
    }

    #[test]
    fn test_variables() {
        let mut ctx = EvalContext::new(1000, 0);
        ctx.set_variable("length", 10);
        ctx.set_variable("offset", 0x100);
        assert_eq!(eval_to_i128("$length + 5", &ctx).unwrap(), 15);
        assert_eq!(eval_to_i128("$offset", &ctx).unwrap(), 256);
        assert_eq!(eval_to_i128("$@file_size", &ctx).unwrap(), 1000);
    }

    #[test]
    fn test_comparisons_and_logic() {
        let mut ctx = EvalContext::new(100, 0);
        ctx.set_variable("version", 2);
        assert!(eval_to_bool("$version >= 2", &ctx).unwrap());
        assert!(!eval_to_bool("$version == 1", &ctx).unwrap());
        assert!(eval_to_bool("$version >= 2 and $version < 5", &ctx).unwrap());
        assert!(eval_to_bool("$version == 1 or $version == 2", &ctx).unwrap());
        assert!(eval_to_bool("not ($version == 1)", &ctx).unwrap());
    }

    #[test]
    fn test_bitfield_variable() {
        let mut ctx = EvalContext::new(100, 0);
        ctx.set_variable("flags.is_compressed", 1);
        assert!(eval_to_bool("$flags.is_compressed", &ctx).unwrap());
    }

    #[test]
    fn test_magic_variables() {
        let ctx = EvalContext::new(1000, 0x100);
        assert_eq!(eval_to_i128("$@file_size", &ctx).unwrap(), 1000);
        assert_eq!(eval_to_i128("$@base", &ctx).unwrap(), 0x100);
    }

    #[test]
    fn test_error_unknown_variable() {
        let ctx = EvalContext::new(100, 0);
        assert!(eval_to_i128("$missing", &ctx).is_err());
    }

    #[test]
    fn test_error_division_by_zero() {
        let ctx = EvalContext::new(100, 0);
        assert!(eval_to_i128("10 / 0", &ctx).is_err());
    }

    #[test]
    fn test_resolve_size_or_offset() {
        let mut ctx = EvalContext::new(100, 0);
        ctx.set_variable("len", 10);
        assert_eq!(resolve_size_or_offset("$len + 2", &ctx).unwrap(), 12);
        assert!(resolve_size_or_offset("-1", &ctx).is_err());
    }
}
