//! The per-poll `requires` expression.
//!
//! ```text
//! expr   := term ('OR' term)*
//! term   := factor ('AND' factor)*
//! factor := 'NOT' factor | '(' expr ')' | class
//! class  := name [ '(' key '=' number (',' key '=' number)* ')' ]
//! ```
//!
//! Operators and class names are case-insensitive. `NOT` binds tighter than
//! `AND`, which binds tighter than `OR`. The expression is parsed and validated
//! when a poll is created, never when somebody votes.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
};

const MAX_EXPRESSION_LENGTH: usize = 300;
const MAX_NESTING: usize = 8;
const MAX_CLASS_CALLS: usize = 12;

/// A player class a poll can require.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Class {
    CrystalPvper,
    Veteran,
    Builder,
    OverallActive,
    Active,
    VeryActive,
}

/// One overridable parameter of a class, with the range staff may use.
pub struct ParamSpec {
    pub key: &'static str,
    pub min: i64,
    pub max: i64,
}

const fn param(key: &'static str, min: i64, max: i64) -> ParamSpec {
    ParamSpec { key, min, max }
}

const VETERAN_PARAMS: &[ParamSpec] = &[param("days", 1, 3650)];
const ACTIVITY_PARAMS: &[ParamSpec] = &[
    param("days", 1, 60),
    param("window", 1, 60),
    param("minutes", 1, 1440),
];
const CRYSTAL_PARAMS: &[ParamSpec] = &[
    param("fights", 1, 1000),
    param("opponents", 1, 100),
    param("damage", 0, 100_000),
    param("window", 1, 45),
];
const BUILDER_PARAMS: &[ParamSpec] = &[
    param("days", 1, 45),
    param("window", 1, 45),
    param("placed", 0, 10_000_000),
    param("materials", 0, 500),
];

impl Class {
    pub const ALL: [Self; 6] = [
        Self::CrystalPvper,
        Self::Veteran,
        Self::Builder,
        Self::OverallActive,
        Self::Active,
        Self::VeryActive,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::CrystalPvper => "crystal_pvper",
            Self::Veteran => "veteran",
            Self::Builder => "builder",
            Self::OverallActive => "overall_active",
            Self::Active => "active",
            Self::VeryActive => "very_active",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|class| class.name().eq_ignore_ascii_case(name))
    }

    pub const fn params(self) -> &'static [ParamSpec] {
        match self {
            Self::Veteran => VETERAN_PARAMS,
            Self::OverallActive | Self::Active | Self::VeryActive => ACTIVITY_PARAMS,
            Self::CrystalPvper => CRYSTAL_PARAMS,
            Self::Builder => BUILDER_PARAMS,
        }
    }
}

/// A class with per-poll overrides of its configured defaults.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ClassCall {
    pub class: Class,
    pub overrides: BTreeMap<String, i64>,
}

impl ClassCall {
    #[cfg(test)]
    pub fn plain(class: Class) -> Self {
        Self {
            class,
            overrides: BTreeMap::new(),
        }
    }

    /// The override for `key`, if staff gave one.
    pub fn get(&self, key: &str) -> Option<i64> {
        self.overrides.get(key).copied()
    }
}

impl fmt::Display for ClassCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.class.name())?;
        if !self.overrides.is_empty() {
            f.write_str("(")?;
            for (index, (key, value)) in self.overrides.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{key}={value}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    Class(ClassCall),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

impl Expr {
    /// Every distinct class call in the expression, in first-use order.
    pub fn class_calls(&self) -> Vec<ClassCall> {
        fn walk(expr: &Expr, seen: &mut HashSet<ClassCall>, out: &mut Vec<ClassCall>) {
            match expr {
                Expr::Class(call) => {
                    if seen.insert(call.clone()) {
                        out.push(call.clone());
                    }
                }
                Expr::Not(inner) => walk(inner, seen, out),
                Expr::And(items) | Expr::Or(items) => {
                    for item in items {
                        walk(item, seen, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(self, &mut HashSet::new(), &mut out);
        out
    }

    pub fn uses_not(&self) -> bool {
        match self {
            Self::Class(_) => false,
            Self::Not(_) => true,
            Self::And(items) | Self::Or(items) => items.iter().any(Self::uses_not),
        }
    }

    /// Evaluates the expression over precomputed class sets. `universe` is the
    /// set `NOT` complements against (every candidate account).
    pub fn evaluate(
        &self,
        sets: &HashMap<ClassCall, HashSet<String>>,
        universe: &HashSet<String>,
    ) -> HashSet<String> {
        match self {
            Self::Class(call) => sets.get(call).cloned().unwrap_or_default(),
            Self::Not(inner) => {
                let excluded = inner.evaluate(sets, universe);
                universe.difference(&excluded).cloned().collect()
            }
            Self::And(items) => {
                let mut iter = items.iter().map(|item| item.evaluate(sets, universe));
                let Some(mut result) = iter.next() else {
                    return HashSet::new();
                };
                for next in iter {
                    result.retain(|value| next.contains(value));
                }
                result
            }
            Self::Or(items) => items
                .iter()
                .flat_map(|item| item.evaluate(sets, universe))
                .collect(),
        }
    }

    /// Checks one account against already-evaluated class membership.
    /// Used by tests and by anything that needs the boolean form.
    #[cfg(test)]
    pub fn matches(&self, has: &impl Fn(&ClassCall) -> bool) -> bool {
        match self {
            Self::Class(call) => has(call),
            Self::Not(inner) => !inner.matches(has),
            Self::And(items) => items.iter().all(|item| item.matches(has)),
            Self::Or(items) => items.iter().any(|item| item.matches(has)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub message: String,
    /// Character offset in the input.
    pub position: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at character {})", self.message, self.position + 1)
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    Word(String),
    Number(i64),
    LParen,
    RParen,
    Comma,
    Equals,
}

fn error<T>(message: impl Into<String>, position: usize) -> Result<T, ParseError> {
    Err(ParseError {
        message: message.into(),
        position,
    })
}

fn tokenize(input: &str) -> Result<Vec<(Token, usize)>, ParseError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        let start = index;
        if character.is_whitespace() {
            index += 1;
        } else if character == '(' {
            tokens.push((Token::LParen, start));
            index += 1;
        } else if character == ')' {
            tokens.push((Token::RParen, start));
            index += 1;
        } else if character == ',' {
            tokens.push((Token::Comma, start));
            index += 1;
        } else if character == '=' {
            tokens.push((Token::Equals, start));
            index += 1;
        } else if character.is_ascii_digit() {
            while index < chars.len() && chars[index].is_ascii_digit() {
                index += 1;
            }
            let text: String = chars[start..index].iter().collect();
            let Ok(value) = text.parse::<i64>() else {
                return error("number is too large", start);
            };
            tokens.push((Token::Number(value), start));
        } else if character.is_ascii_alphabetic() || character == '_' {
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
            {
                index += 1;
            }
            let word: String = chars[start..index].iter().collect();
            tokens.push((Token::Word(word.to_ascii_lowercase()), start));
        } else {
            return error(format!("unexpected character '{character}'"), start);
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    index: usize,
    end: usize,
    calls: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index).map(|(token, _)| token)
    }

    fn position(&self) -> usize {
        self.tokens
            .get(self.index)
            .map_or(self.end, |(_, position)| *position)
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(found)) if found == word)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).map(|(token, _)| token.clone());
        if token.is_some() {
            self.index += 1;
        }
        token
    }

    fn expect(&mut self, wanted: &Token, description: &str) -> Result<(), ParseError> {
        let position = self.position();
        if self.peek() == Some(wanted) {
            self.index += 1;
            Ok(())
        } else {
            error(format!("expected {description}"), position)
        }
    }

    fn expr(&mut self, depth: usize) -> Result<Expr, ParseError> {
        if depth > MAX_NESTING {
            return error("expression is nested too deeply", self.position());
        }
        let mut items = vec![self.term(depth)?];
        while self.is_word("or") {
            self.index += 1;
            items.push(self.term(depth)?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::Or(items)
        })
    }

    fn term(&mut self, depth: usize) -> Result<Expr, ParseError> {
        let mut items = vec![self.factor(depth)?];
        while self.is_word("and") {
            self.index += 1;
            items.push(self.factor(depth)?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::And(items)
        })
    }

    fn factor(&mut self, depth: usize) -> Result<Expr, ParseError> {
        if depth > MAX_NESTING {
            return error("expression is nested too deeply", self.position());
        }
        let position = self.position();
        match self.next() {
            Some(Token::Word(word)) if word == "not" => {
                Ok(Expr::Not(Box::new(self.factor(depth + 1)?)))
            }
            Some(Token::LParen) => {
                let inner = self.expr(depth + 1)?;
                self.expect(&Token::RParen, "a closing parenthesis")?;
                Ok(inner)
            }
            Some(Token::Word(word)) if word == "and" || word == "or" => {
                error(format!("'{word}' needs a class on its left"), position)
            }
            Some(Token::Word(word)) => self.class(&word, position),
            Some(_) => error("expected a class name or '('", position),
            None => error(
                "the expression ended early; expected a class name",
                position,
            ),
        }
    }

    fn class(&mut self, name: &str, position: usize) -> Result<Expr, ParseError> {
        let Some(class) = Class::from_name(name) else {
            let known: Vec<_> = Class::ALL.iter().map(|class| class.name()).collect();
            return error(
                format!("unknown class '{name}' (known: {})", known.join(", ")),
                position,
            );
        };
        self.calls += 1;
        if self.calls > MAX_CLASS_CALLS {
            return error(
                format!("too many classes (at most {MAX_CLASS_CALLS})"),
                position,
            );
        }
        let mut overrides = BTreeMap::new();
        if self.peek() == Some(&Token::LParen) {
            self.index += 1;
            loop {
                let key_position = self.position();
                let Some(Token::Word(key)) = self.next() else {
                    return error("expected a parameter name", key_position);
                };
                let Some(spec) = class.params().iter().find(|spec| spec.key == key) else {
                    let known: Vec<_> = class.params().iter().map(|spec| spec.key).collect();
                    return error(
                        format!(
                            "class '{}' has no parameter '{key}' (known: {})",
                            class.name(),
                            known.join(", ")
                        ),
                        key_position,
                    );
                };
                self.expect(&Token::Equals, "'='")?;
                let value_position = self.position();
                let Some(Token::Number(value)) = self.next() else {
                    return error("expected a whole number", value_position);
                };
                if value < spec.min || value > spec.max {
                    return error(
                        format!("'{key}' must be between {} and {}", spec.min, spec.max),
                        value_position,
                    );
                }
                if overrides.insert(key.clone(), value).is_some() {
                    return error(format!("'{key}' is given twice"), key_position);
                }
                let separator_position = self.position();
                match self.next() {
                    Some(Token::Comma) => {}
                    Some(Token::RParen) => break,
                    _ => return error("expected ',' or ')'", separator_position),
                }
            }
        }
        Ok(Expr::Class(ClassCall { class, overrides }))
    }
}

/// Parses and validates a `requires` expression.
pub fn parse(input: &str) -> Result<Expr, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return error("the requires expression is empty", 0);
    }
    if trimmed.chars().count() > MAX_EXPRESSION_LENGTH {
        return error(
            format!("the requires expression is longer than {MAX_EXPRESSION_LENGTH} characters"),
            MAX_EXPRESSION_LENGTH,
        );
    }
    let tokens = tokenize(trimmed)?;
    let mut parser = Parser {
        tokens,
        index: 0,
        end: trimmed.chars().count(),
        calls: 0,
    };
    let expr = parser.expr(0)?;
    if parser.index < parser.tokens.len() {
        let position = parser.position();
        return error(
            "unexpected input; expected AND, OR or the end of the expression",
            position,
        );
    }
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(class: Class) -> Expr {
        Expr::Class(ClassCall::plain(class))
    }

    #[test]
    fn parses_a_single_class() {
        assert_eq!(parse("crystal_pvper").unwrap(), class(Class::CrystalPvper));
        assert_eq!(parse("  VETERAN  ").unwrap(), class(Class::Veteran));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let parsed = parse("builder OR veteran AND active").unwrap();
        assert_eq!(
            parsed,
            Expr::Or(vec![
                class(Class::Builder),
                Expr::And(vec![class(Class::Veteran), class(Class::Active)]),
            ])
        );
    }

    #[test]
    fn parentheses_override_precedence() {
        let parsed = parse("(builder OR veteran) AND active").unwrap();
        assert_eq!(
            parsed,
            Expr::And(vec![
                Expr::Or(vec![class(Class::Builder), class(Class::Veteran)]),
                class(Class::Active),
            ])
        );
    }

    #[test]
    fn not_binds_tighter_than_and() {
        let parsed = parse("active and not builder").unwrap();
        assert_eq!(
            parsed,
            Expr::And(vec![
                class(Class::Active),
                Expr::Not(Box::new(class(Class::Builder)))
            ])
        );
        let grouped = parse("NOT (builder OR veteran)").unwrap();
        assert_eq!(
            grouped,
            Expr::Not(Box::new(Expr::Or(vec![
                class(Class::Builder),
                class(Class::Veteran)
            ])))
        );
    }

    #[test]
    fn parses_class_overrides() {
        let parsed = parse("veteran(days=365) AND very_active(days = 5, minutes=90)").unwrap();
        let Expr::And(items) = parsed else {
            panic!("expected AND");
        };
        let Expr::Class(veteran) = &items[0] else {
            panic!("expected class");
        };
        assert_eq!(veteran.get("days"), Some(365));
        let Expr::Class(active) = &items[1] else {
            panic!("expected class");
        };
        assert_eq!(active.class, Class::VeryActive);
        assert_eq!(active.get("days"), Some(5));
        assert_eq!(active.get("minutes"), Some(90));
        assert_eq!(active.get("window"), None);
    }

    #[test]
    fn rejects_unknown_classes_and_keys() {
        let unknown = parse("crystal_pvper OR whale").unwrap_err();
        assert!(unknown.message.contains("unknown class 'whale'"));
        assert_eq!(unknown.position, 17);
        let key = parse("veteran(age=3)").unwrap_err();
        assert!(key.message.contains("no parameter 'age'"));
        let range = parse("veteran(days=0)").unwrap_err();
        assert!(range.message.contains("between"));
        let twice = parse("veteran(days=1, days=2)").unwrap_err();
        assert!(twice.message.contains("twice"));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "   ",
            "AND veteran",
            "veteran AND",
            "veteran veteran",
            "veteran OR OR active",
            "(veteran",
            "veteran)",
            "veteran(days=)",
            "veteran(days=1",
            "veteran(days 1)",
            "veteran()",
            "NOT",
            "veteran && active",
            "veteran(days=99999999999999999999)",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn rejects_runaway_expressions() {
        let deep = format!("{}veteran{}", "(".repeat(20), ")".repeat(20));
        assert!(parse(&deep).unwrap_err().message.contains("nested"));
        let long = vec!["veteran"; 40].join(" OR ");
        assert!(parse(&long).is_err());
        let many = vec!["veteran"; 13].join(" AND ");
        assert!(parse(&many).unwrap_err().message.contains("too many"));
    }

    #[test]
    fn class_calls_are_deduplicated_in_first_use_order() {
        let parsed = parse("veteran AND (active OR veteran) AND veteran(days=9)").unwrap();
        let calls = parsed.class_calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0], ClassCall::plain(Class::Veteran));
        assert_eq!(calls[1], ClassCall::plain(Class::Active));
        assert_eq!(calls[2].get("days"), Some(9));
    }

    fn set(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn evaluates_set_algebra() {
        let parsed = parse("(veteran AND active) OR builder").unwrap();
        let mut sets = HashMap::new();
        sets.insert(ClassCall::plain(Class::Veteran), set(&["a", "b", "c"]));
        sets.insert(ClassCall::plain(Class::Active), set(&["b", "c", "d"]));
        sets.insert(ClassCall::plain(Class::Builder), set(&["e"]));
        let universe = set(&["a", "b", "c", "d", "e", "f"]);
        assert_eq!(parsed.evaluate(&sets, &universe), set(&["b", "c", "e"]));

        let negated = parse("active AND NOT veteran").unwrap();
        assert_eq!(negated.evaluate(&sets, &universe), set(&["d"]));
        let alone = parse("NOT active").unwrap();
        assert_eq!(alone.evaluate(&sets, &universe), set(&["a", "e", "f"]));
        assert!(alone.uses_not());
        assert!(!parsed.uses_not());
    }

    #[test]
    fn boolean_form_agrees_with_set_form() {
        let parsed = parse("(veteran OR builder) AND NOT active").unwrap();
        let veteran = set(&["a", "b"]);
        let builder = set(&["c"]);
        let active = set(&["b"]);
        for account in ["a", "b", "c", "d"] {
            let matches = parsed.matches(&|call| match call.class {
                Class::Veteran => veteran.contains(account),
                Class::Builder => builder.contains(account),
                Class::Active => active.contains(account),
                _ => false,
            });
            assert_eq!(matches, matches!(account, "a" | "c"), "account {account}");
        }
    }

    #[test]
    fn display_round_trips_class_calls() {
        let parsed = parse("crystal_pvper(window=10, fights=3)").unwrap();
        let Expr::Class(call) = parsed else {
            panic!("expected class");
        };
        assert_eq!(call.to_string(), "crystal_pvper(fights=3, window=10)");
    }
}
