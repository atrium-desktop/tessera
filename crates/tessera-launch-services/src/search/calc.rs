//! Lightweight, zero-dependency safe inline arithmetic evaluator for the Intent engine.

#[derive(Debug, PartialEq)]
pub enum Token {
    Number(f64),
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
    Power,
    LParen,
    RParen,
}

pub fn tokenize(expr: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = expr.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' | '×' => {
                tokens.push(Token::Multiply);
                chars.next();
            }
            '/' | '÷' => {
                tokens.push(Token::Divide);
                chars.next();
            }
            '%' => {
                tokens.push(Token::Modulo);
                chars.next();
            }
            '^' => {
                tokens.push(Token::Power);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            '0'..='9' | '.' => {
                let mut num_str = String::new();
                let mut has_dot = false;
                while let Some(&nc) = chars.peek() {
                    if nc.is_ascii_digit() {
                        num_str.push(nc);
                        chars.next();
                    } else if nc == '.' && !has_dot {
                        has_dot = true;
                        num_str.push(nc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let val: f64 = num_str.parse().ok()?;
                tokens.push(Token::Number(val));
            }
            _ => return None,
        }
    }

    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next_token(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos);
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn parse_expr(&mut self) -> Option<f64> {
        let mut left = self.parse_term()?;

        while let Some(tok) = self.peek() {
            match tok {
                Token::Plus => {
                    self.next_token();
                    let right = self.parse_term()?;
                    left += right;
                }
                Token::Minus => {
                    self.next_token();
                    let right = self.parse_term()?;
                    left -= right;
                }
                _ => break,
            }
        }

        Some(left)
    }

    fn parse_term(&mut self) -> Option<f64> {
        let mut left = self.parse_factor()?;

        while let Some(tok) = self.peek() {
            match tok {
                Token::Multiply => {
                    self.next_token();
                    let right = self.parse_factor()?;
                    left *= right;
                }
                Token::Divide => {
                    self.next_token();
                    let right = self.parse_factor()?;
                    if right == 0.0 {
                        return None;
                    }
                    left /= right;
                }
                Token::Modulo => {
                    self.next_token();
                    let right = self.parse_factor()?;
                    if right == 0.0 {
                        return None;
                    }
                    left %= right;
                }
                _ => break,
            }
        }

        Some(left)
    }

    fn parse_factor(&mut self) -> Option<f64> {
        let left = self.parse_primary()?;

        if let Some(Token::Power) = self.peek() {
            self.next_token();
            let right = self.parse_factor()?; // Right-associative
            Some(left.powf(right))
        } else {
            Some(left)
        }
    }

    fn parse_primary(&mut self) -> Option<f64> {
        match self.next_token()? {
            Token::Number(n) => Some(*n),
            Token::Minus => {
                let val = self.parse_primary()?;
                Some(-val)
            }
            Token::Plus => self.parse_primary(),
            Token::LParen => {
                let val = self.parse_expr()?;
                if let Some(Token::RParen) = self.next_token() {
                    Some(val)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Try evaluating `expr` as an arithmetic expression.
/// Returns `Some(formatted_result)` only if `expr` contains mathematical
/// operators or structure (not just a single number) and evaluates successfully.
pub fn eval_math(expr: &str) -> Option<String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Require at least one arithmetic operator to avoid intercepting plain numbers
    let has_operator = trimmed
        .chars()
        .any(|c| matches!(c, '+' | '-' | '*' | '×' | '/' | '÷' | '%' | '^'));
    if !has_operator {
        return None;
    }

    let tokens = tokenize(trimmed)?;
    // If it only tokenized into a single signed number (e.g. "+5" or "-5"), do not intercept
    if tokens.len() <= 2
        && tokens.iter().any(|t| matches!(t, Token::Number(_)))
        && !tokens.iter().any(|t| {
            matches!(
                t,
                Token::Multiply | Token::Divide | Token::Power | Token::Modulo
            )
        })
    {
        return None;
    }

    let mut parser = Parser::new(tokens);
    let val = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() || val.is_nan() || val.is_infinite() {
        return None;
    }

    // Format integer vs decimal nicely
    if (val.fract()).abs() < 1e-9 && val.abs() < 1e15 {
        Some(format!("{}", val as i64))
    } else {
        Some(
            format!("{:.4}", val)
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_eval_cases() {
        assert_eq!(eval_math("1 + 1"), Some("2".into()));
        assert_eq!(eval_math("1920 * 1080"), Some("2073600".into()));
        assert_eq!(eval_math("100 / 4"), Some("25".into()));
        assert_eq!(eval_math("2 ^ 10"), Some("1024".into()));
        assert_eq!(eval_math("(3 + 5) * 2"), Some("16".into()));
        assert_eq!(eval_math("3.5 * 2"), Some("7".into()));
        assert_eq!(eval_math("10 % 3"), Some("1".into()));
        assert_eq!(eval_math("5 × 8"), Some("40".into()));
        assert_eq!(eval_math("100 ÷ 5"), Some("20".into()));
    }

    #[test]
    fn non_math_rejected() {
        assert_eq!(eval_math("firefox"), None);
        assert_eq!(eval_math("123"), None);
        assert_eq!(eval_math(""), None);
        assert_eq!(eval_math("1 / 0"), None);
        assert_eq!(eval_math("+5"), None);
        assert_eq!(eval_math("-42"), None);
    }
}
