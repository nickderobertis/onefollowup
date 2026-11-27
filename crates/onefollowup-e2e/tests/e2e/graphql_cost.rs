//! GitHub's published GraphQL rate-limit calculation, over a document's own text and the
//! variables it was sent with: every connection a request selects (a field given `first` or
//! `last`) needs as many requests as its parents' page sizes multiply to — one for a
//! connection at the top — and the sum of those requests, divided by 100 and rounded, is the
//! request's cost in points, at least 1.
//!
//! <https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api>
//!
//! Written for the loopback rather than borrowed from onetaskgraph, so
//! the loopback's price is a second reading of the documents onetaskgraph sends, not its own.

use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Name(String),
    Punct(char),
    Spread,
    Int(i64),
    Str(String),
}

fn tokens(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() || c == ',' || c == '\u{feff}' {
            at += 1;
        } else if c == '#' {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if c == '.' && chars.get(at + 1) == Some(&'.') && chars.get(at + 2) == Some(&'.') {
            out.push(Token::Spread);
            at += 3;
        } else if c == '"' {
            let mut value = String::new();
            at += 1;
            while at < chars.len() && chars[at] != '"' {
                if chars[at] == '\\' {
                    at += 1;
                }
                value.push(chars[at]);
                at += 1;
            }
            at += 1;
            out.push(Token::Str(value));
        } else if c == '-' || c.is_ascii_digit() {
            let start = at;
            at += 1;
            while at < chars.len() && (chars[at].is_ascii_digit() || chars[at] == '.') {
                at += 1;
            }
            let text: String = chars[start..at].iter().collect();
            out.push(Token::Int(text.parse::<f64>().unwrap_or(0.0) as i64));
        } else if c == '_' || c.is_ascii_alphabetic() {
            let start = at;
            while at < chars.len() && (chars[at] == '_' || chars[at].is_ascii_alphanumeric()) {
                at += 1;
            }
            out.push(Token::Name(chars[start..at].iter().collect()));
        } else {
            out.push(Token::Punct(c));
            at += 1;
        }
    }
    out
}

#[derive(Debug, Clone)]
enum Arg {
    Variable(String),
    Int(i64),
    Bool(bool),
    Other,
}

#[derive(Debug, Clone)]
struct Directive {
    name: String,
    condition: Option<Arg>,
}

#[derive(Debug, Clone)]
enum Selection {
    Field {
        args: Vec<(String, Arg)>,
        directives: Vec<Directive>,
        selections: Vec<Selection>,
    },
    Spread {
        name: String,
        directives: Vec<Directive>,
    },
    Inline {
        directives: Vec<Directive>,
        selections: Vec<Selection>,
    },
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }
    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Token::Punct(c)) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn name(&mut self) -> String {
        match self.next() {
            Some(Token::Name(name)) => name,
            other => panic!("a GraphQL name, not {other:?}"),
        }
    }
    /// Skip one balanced group opened by `open` (already current).
    fn skip_group(&mut self, open: char, close: char) {
        assert!(self.eat(open));
        let mut depth = 1;
        while depth > 0 {
            match self.next() {
                Some(Token::Punct(c)) if c == open => depth += 1,
                Some(Token::Punct(c)) if c == close => depth -= 1,
                Some(_) => {}
                None => panic!("an unbalanced {open}"),
            }
        }
    }
    fn value(&mut self) -> Arg {
        match self.peek().cloned() {
            Some(Token::Punct('$')) => {
                self.at += 1;
                Arg::Variable(self.name())
            }
            Some(Token::Int(value)) => {
                self.at += 1;
                Arg::Int(value)
            }
            Some(Token::Name(name)) => {
                self.at += 1;
                match name.as_str() {
                    "true" => Arg::Bool(true),
                    "false" => Arg::Bool(false),
                    _ => Arg::Other,
                }
            }
            Some(Token::Str(_)) => {
                self.at += 1;
                Arg::Other
            }
            Some(Token::Punct('[')) => {
                self.skip_group('[', ']');
                Arg::Other
            }
            Some(Token::Punct('{')) => {
                self.skip_group('{', '}');
                Arg::Other
            }
            other => panic!("a GraphQL value, not {other:?}"),
        }
    }
    fn args(&mut self) -> Vec<(String, Arg)> {
        let mut args = Vec::new();
        if self.eat('(') {
            while !self.eat(')') {
                let name = self.name();
                assert!(self.eat(':'), "an argument's colon");
                args.push((name, self.value()));
            }
        }
        args
    }
    fn directives(&mut self) -> Vec<Directive> {
        let mut directives = Vec::new();
        while self.eat('@') {
            let name = self.name();
            let condition = self
                .args()
                .into_iter()
                .find(|(name, _)| name == "if")
                .map(|(_, value)| value);
            directives.push(Directive { name, condition });
        }
        directives
    }
    fn selections(&mut self) -> Vec<Selection> {
        let mut out = Vec::new();
        assert!(self.eat('{'), "a selection set");
        while !self.eat('}') {
            if self.peek() == Some(&Token::Spread) {
                self.at += 1;
                match self.peek().cloned() {
                    Some(Token::Name(name)) if name == "on" => {
                        self.at += 1;
                        self.name();
                        let directives = self.directives();
                        let selections = self.selections();
                        out.push(Selection::Inline {
                            directives,
                            selections,
                        });
                    }
                    Some(Token::Name(name)) => {
                        self.at += 1;
                        let directives = self.directives();
                        out.push(Selection::Spread { name, directives });
                    }
                    _ => {
                        let directives = self.directives();
                        let selections = self.selections();
                        out.push(Selection::Inline {
                            directives,
                            selections,
                        });
                    }
                }
                continue;
            }
            let _alias_or_name = self.name();
            if self.eat(':') {
                self.name();
            }
            let args = self.args();
            let directives = self.directives();
            let selections = if self.peek() == Some(&Token::Punct('{')) {
                self.selections()
            } else {
                Vec::new()
            };
            out.push(Selection::Field {
                args,
                directives,
                selections,
            });
        }
        out
    }
}

/// A parsed document: its operations' selections and its fragments.
struct Document {
    operations: Vec<Vec<Selection>>,
    fragments: BTreeMap<String, Vec<Selection>>,
}

fn parse(text: &str) -> Document {
    let mut parser = Parser {
        tokens: tokens(text),
        at: 0,
    };
    let mut operations = Vec::new();
    let mut fragments = BTreeMap::new();
    while let Some(token) = parser.peek().cloned() {
        match token {
            Token::Punct('{') => operations.push(parser.selections()),
            Token::Name(word) if word == "fragment" => {
                parser.at += 1;
                let name = parser.name();
                assert_eq!(parser.name(), "on");
                parser.name();
                parser.directives();
                fragments.insert(name, parser.selections());
            }
            Token::Name(_) => {
                parser.at += 1;
                if let Some(Token::Name(_)) = parser.peek() {
                    parser.at += 1;
                }
                if parser.peek() == Some(&Token::Punct('(')) {
                    parser.skip_group('(', ')');
                }
                parser.directives();
                operations.push(parser.selections());
            }
            other => panic!("a GraphQL definition, not {other:?}"),
        }
    }
    Document {
        operations,
        fragments,
    }
}

fn resolve(arg: &Arg, variables: &Value) -> Option<i64> {
    match arg {
        Arg::Int(value) => Some(*value),
        Arg::Variable(name) => variables.get(name).and_then(Value::as_i64),
        _ => None,
    }
}

fn included(directives: &[Directive], variables: &Value) -> bool {
    directives.iter().all(|directive| {
        let holds = match &directive.condition {
            Some(Arg::Bool(value)) => *value,
            Some(Arg::Variable(name)) => variables.get(name).and_then(Value::as_bool) == Some(true),
            _ => true,
        };
        match directive.name.as_str() {
            "include" => holds,
            "skip" => !holds,
            _ => true,
        }
    })
}

fn requests(
    selections: &[Selection],
    multiplier: u64,
    document: &Document,
    variables: &Value,
) -> u64 {
    let mut total = 0;
    for selection in selections {
        match selection {
            Selection::Field {
                args,
                directives,
                selections,
            } => {
                if !included(directives, variables) {
                    continue;
                }
                let page = args
                    .iter()
                    .find(|(name, _)| name == "first" || name == "last")
                    .and_then(|(_, arg)| resolve(arg, variables));
                let inner = match page {
                    Some(page) => {
                        total += multiplier;
                        multiplier * u64::try_from(page.max(0)).unwrap_or(0)
                    }
                    None => multiplier,
                };
                total += requests(selections, inner, document, variables);
            }
            Selection::Spread { name, directives } => {
                if included(directives, variables) {
                    let fragment = document
                        .fragments
                        .get(name)
                        .unwrap_or_else(|| panic!("the fragment {name}"));
                    total += requests(fragment, multiplier, document, variables);
                }
            }
            Selection::Inline {
                directives,
                selections,
            } => {
                if included(directives, variables) {
                    total += requests(selections, multiplier, document, variables);
                }
            }
        }
    }
    total
}

/// The connection requests `document` needs under `variables`.
pub fn connection_requests(document: &str, variables: &Value) -> u64 {
    let parsed = parse(document);
    parsed
        .operations
        .iter()
        .map(|operation| requests(operation, 1, &parsed, variables))
        .sum()
}

/// What `document` sent with `variables` costs, in points: its connection requests divided
/// by 100, rounded, at least 1.
pub fn points(document: &str, variables: &Value) -> u64 {
    let requests = connection_requests(document, variables);
    ((requests as f64 / 100.0).round() as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// GitHub's own worked example: 1 + 100 + 5,000 requests, 51 points.
    #[test]
    fn github_s_worked_example_costs_51_points() {
        let document = "query { viewer { login repositories(first: 100) { edges { node { name pullRequests(first: 50) { edges { node { title comments(first: 10) { edges { node { bodyHTML } } } } } } } } } } }";
        // repositories: 1; pullRequests: 100; comments: 100 * 50 = 5,000.
        assert_eq!(connection_requests(document, &json!({})), 5101);
        assert_eq!(points(document, &json!({})), 51);
    }
}
