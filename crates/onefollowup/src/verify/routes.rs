//! Routes: matched in order against a ticket's kind, severity, repository and paths, the
//! first match winning; its `set` applied over the kind's defaults, and its `command` run
//! before the ticket's write.
//!
//! A route's `match.relations` and `set.assignee` are the relations' to decide, which this
//! release does not implement: a `relations` condition is not evaluated, and an assignee is
//! not written.

use crate::config::{Route, RouteMatch};

/// What a ticket is matched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts<'a> {
    /// Its kind.
    pub kind: &'a str,
    /// The rubric severity it records.
    pub severity: Option<&'a str>,
    /// The repository its item is filed in.
    pub repository: &'a str,
    /// The paths its drafts name.
    pub paths: &'a [String],
}

/// The first route `facts` match, with its position.
#[must_use]
pub fn first<'a>(routes: &'a [Route], facts: &Facts<'_>) -> Option<(usize, &'a Route)> {
    routes
        .iter()
        .enumerate()
        .find(|(_, route)| matches(&route.matches, facts))
}

fn matches(condition: &RouteMatch, facts: &Facts<'_>) -> bool {
    let kind = condition
        .kind
        .as_ref()
        .is_none_or(|kinds| kinds.iter().any(|kind| kind == facts.kind));
    let severity = condition.severity.as_ref().is_none_or(|severities| {
        facts
            .severity
            .is_some_and(|severity| severities.iter().any(|one| one == severity))
    });
    let repository = condition
        .repository
        .as_ref()
        .is_none_or(|globs| globs.iter().any(|glob| glob_match(glob, facts.repository)));
    let path = condition.path.as_ref().is_none_or(|globs| {
        facts
            .paths
            .iter()
            .any(|path| globs.iter().any(|glob| glob_match(glob, path)))
    });
    kind && severity && repository && path
}

/// Whether `text` matches `glob`: `**` any run of characters, `*` any run without a `/`, `?`
/// one character other than `/`, everything else itself.
#[must_use]
pub fn glob_match(glob: &str, text: &str) -> bool {
    let glob: Vec<char> = glob.chars().collect();
    let text: Vec<char> = text.chars().collect();
    matches_from(&glob, &text)
}

fn matches_from(glob: &[char], text: &[char]) -> bool {
    match glob.first() {
        None => text.is_empty(),
        Some('*') if glob.get(1) == Some(&'*') => {
            let rest = glob[2..].strip_prefix(&['/']).unwrap_or(&glob[2..]);
            (0..=text.len()).any(|at| matches_from(rest, &text[at..]))
                || (0..=text.len()).any(|at| matches_from(&glob[2..], &text[at..]))
        }
        Some('*') => (0..=text.len())
            .take_while(|at| *at == 0 || text[at - 1] != '/')
            .any(|at| matches_from(&glob[1..], &text[at..])),
        Some('?') => text
            .first()
            .is_some_and(|one| *one != '/' && matches_from(&glob[1..], &text[1..])),
        Some(one) => text.first() == Some(one) && matches_from(&glob[1..], &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn a_glob_matches_by_segment_and_across_segments() {
        assert!(glob_match(
            "github.com/example/*",
            "github.com/example/widgets"
        ));
        assert!(!glob_match(
            "github.com/example/*",
            "github.com/example/a/b"
        ));
        assert!(glob_match("src/**", "src/a/b.rs"));
        assert!(glob_match("src/**/b.rs", "src/b.rs"));
        assert!(glob_match("src/?.rs", "src/a.rs"));
        assert!(!glob_match("src/?.rs", "src/ab.rs"));
        assert!(!glob_match("docs/**", "src/a.rs"));
    }
}
