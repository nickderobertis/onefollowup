//! The 50-draft batch and boards of `prepare-graphql-points-per-draft` and
//! `prepare-seconds-per-batch` (`budgets.yaml`), at the real `followups` board's shape as
//! `spike-real-board-cost` measured it: a public board of 2,138 issues, 465 open, whose body
//! sizes, labels and memberships are the real board's and whose searched query texts match as
//! many issues as the real board's did, beside a 400-ticket private board, 95 open.
//!
//! The drafts are the 13 real clusters of `fixtures/filing-duplicates.json` (their earlier
//! members on the public board), 27 generated public drafts and 10 generated private ones. A
//! generated draft's five terms are five words of its title, each carried by as many public
//! issues as [`words`] designs, so the texts the batch searches spread over the real board's
//! hits per query text ([`REAL_HITS`]); its file names are carried as the shipped crowd carries
//! them.

use crate::duplicate_boards::{REPOSITORIES, clusters, origin};
use crate::loopback_boards::{Board, BoardIssue};
use crate::prepare_cost::{
    PRIVATE_REPOSITORIES, private_board as private_board_of, private_origin,
    public_board as public_board_of,
};
use crate::world::World;

/// The batch every draft is recorded under.
pub const BATCH: &str = "nightly-2026-11-25";
/// The real board's issues, and how many of them are open.
pub const PUBLIC_TICKETS: usize = 2138;
pub const PUBLIC_OPEN: usize = 465;
/// The private board's.
pub const PRIVATE_TICKETS: usize = 400;
pub const PRIVATE_OPEN: usize = 95;
/// The issues carrying one label: every other carries none.
pub const LABELLED: usize = 22;
/// Body bytes at the quantiles measured on the real board: the least, the median, the 90th
/// and 99th percentiles, and the most.
pub const BODY_QUANTILES: [(f64, usize); 5] = [
    (0.0, 2030),
    (0.5, 4498),
    (0.9, 5823),
    (0.99, 8611),
    (1.0, 40249),
];
/// Hits per searched query text on the real board (`hits-per-query.json`, 74 texts): how many
/// texts matched at least the first figure and fewer than the second.
pub const REAL_HITS: [(usize, usize, usize); 5] = [
    (0, 1, 11),
    (1, 21, 19),
    (21, 100, 20),
    (100, 1000, 23),
    (1000, usize::MAX, 1),
];
/// The most issues one real query text matched (`verification`), and the median text's.
pub const MOST_HITS: usize = 1305;
pub const MEDIAN_HITS: f64 = 44.5;

/// The file names generated drafts name, each carried by as many public issues as it says.
const FILES: [(&str, usize); 6] = [
    ("mod.rs", 420),
    ("lib.rs", 300),
    ("main.rs", 260),
    ("config.rs", 200),
    ("cli.rs", 180),
    ("engine.rs", 150),
];
/// How many generated public drafts there are, and private ones.
const GENERATED_PUBLIC: usize = 27;
const GENERATED_PRIVATE: usize = 10;
/// The words a generated draft's title carries: its five terms.
const TERMS: usize = 5;

/// What pads a body to its size: a word no draft's query starts.
const PADDING: &str = "lorem ";

/// Every word a generated public draft searches, each with how many public issues carry it:
/// three common ones (the real board's `verification` at 1,305 and two more past 1,000), 68
/// between 100 and 999, spread geometrically, and 64 between 21 and 99, evenly — so with the
/// titles, which match nothing, and the clusters' own terms, which a handful each carry
/// ([`cluster_terms`]), the texts the batch searches spread as the real board's did. Each word
/// is six letters, no other word of the boards or the drafts begins with it, and it is no
/// stoplisted word.
pub fn words() -> Vec<(String, usize)> {
    let mut counts: Vec<usize> = vec![MOST_HITS, 1180, 1060];
    for at in 0..68 {
        counts.push((100.0 * (999.0_f64 / 100.0).powf(f64::from(at) / 67.0)).round() as usize);
    }
    for at in 0..64 {
        counts.push(21 + (78 * at) / 63);
    }
    let taken = taken_words();
    let consonants = b"bdfgklmnprstvz";
    let vowels = b"aeiou";
    let mut spelled = Vec::new();
    let mut at: usize = 0;
    while spelled.len() < counts.len() {
        // A walk over every consonant-vowel word of six letters, in an order no alphabet sets.
        let mut code = at.wrapping_mul(7919) % (14 * 5 * 14 * 5 * 14 * 5);
        at += 1;
        let mut word = String::new();
        for _ in 0..3 {
            word.push(char::from(consonants[code % 14]));
            code /= 14;
            word.push(char::from(vowels[code % 5]));
            code /= 5;
        }
        if spelled.contains(&word) || taken.iter().any(|held| held.starts_with(word.as_str())) {
            continue;
        }
        spelled.push(word);
    }
    spelled.into_iter().zip(counts).collect()
}

/// Each cluster draft's terms and file names, as prepare plans them, with how many crowd issues
/// carry each beside the cluster's own members: between 1 and 16, as the real board's rarer
/// query texts matched.
fn cluster_terms() -> Vec<(String, usize)> {
    let mut terms: Vec<String> = Vec::new();
    for cluster in clusters() {
        let input: onefollowup::formats::batch::Input = serde_json::from_value(serde_json::json!({
            "id": "drafts:planned",
            "source": "draft",
            "title": cluster.draft.title,
            "repository": origin(&cluster.repository),
            "paths": cluster.draft.paths,
            "body": format!("{}\n", cluster.draft.body),
        }))
        .expect("a draft's input");
        for query in onefollowup::verify::candidates::plan(&input).queries {
            if query.kind != onefollowup::verify::candidates::QueryKind::Title
                && !terms.contains(&query.text)
            {
                terms.push(query.text);
            }
        }
    }
    terms
        .into_iter()
        .enumerate()
        .map(|(at, term)| (term, 1 + (at * 7) % 16))
        .collect()
}

/// Every word the boards and the drafts carry besides the generated ones, and the stoplist's.
fn taken_words() -> Vec<String> {
    let mut texts: Vec<String> = vec![
        PADDING.to_owned(),
        include_str!("../../../onefollowup/defaults/duplicate-stoplist.txt").to_owned(),
    ];
    for cluster in clusters() {
        texts.push(cluster.draft.title.clone());
        texts.push(cluster.draft.body.clone());
        for member in cluster.earlier {
            texts.push(member.title);
            texts.push(member.body);
        }
    }
    let crowd = crate::duplicate_boards::crowd_tickets(
        &REPOSITORIES,
        &crate::duplicate_boards::Crowd {
            count: 30,
            open: 0,
            words: &[],
        },
    );
    for ticket in crowd {
        texts.push(ticket.title);
        texts.push(ticket.body);
        texts.push(ticket.root_cause);
    }
    texts
        .iter()
        .flat_map(|text| {
            text.split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The body size of the issue of `rank` among `issues` sizes, smallest first: linear between
/// the measured quantiles up to the 99th percentile; one issue, the largest, at the measured
/// most; and the few between rising from the 99th percentile to a tenth above it, which the
/// measurement does not say.
pub fn body_size(rank: usize, issues: usize) -> usize {
    let (most, p99) = (BODY_QUANTILES[4].1, BODY_QUANTILES[3]);
    if rank + 1 == issues {
        return most;
    }
    let quantile = rank as f64 / (issues - 1) as f64;
    let at = BODY_QUANTILES
        .windows(2)
        .find(|pair| quantile <= pair[1].0)
        .unwrap_or(&BODY_QUANTILES[3..5]);
    let ((from, low), (to, high)) = if quantile <= p99.0 {
        (at[0], at[1])
    } else {
        (
            p99,
            (
                (issues - 2) as f64 / (issues - 1) as f64,
                p99.1 + p99.1 / 10,
            ),
        )
    };
    let share = ((quantile - from) / (to - from)).clamp(0.0, 1.0);
    (low as f64 + share * (high as f64 - low as f64)).round() as usize
}

/// `issue` with its visible text padded so its whole body is `size` bytes: the padding goes
/// before onetaskgraph's metadata slot, which ends every body.
fn sized(issue: BoardIssue, size: usize) -> BoardIssue {
    let body = issue.body.clone();
    if body.len() >= size {
        return issue;
    }
    let slot = body
        .find("\n\n<!-- onetaskgraph.metadata")
        .unwrap_or(body.len());
    let missing = size - body.len() - 2;
    let mut padding = PADDING.repeat(missing / PADDING.len() + 1);
    padding.truncate(missing);
    let padded = format!("{}\n\n{}{}", &body[..slot], padding, &body[slot..]);
    issue.with_body(padded)
}

/// The words and the file names, as the crowd generator takes them, each count scaled by
/// `scale`'s numerator over its denominator.
fn carried(words: &[(String, usize)], scale: (usize, usize)) -> Vec<(String, usize)> {
    words
        .iter()
        .map(|(word, many)| (word.clone(), *many))
        .chain(FILES.iter().map(|(file, many)| ((*file).to_owned(), *many)))
        .chain(cluster_terms())
        .map(|(word, many)| (word, many * scale.0 / scale.1))
        .collect()
}

/// The public board, project 7 of `example-org`: the clusters' earlier members among a crowd
/// of 2,138 issues in all, 465 open, carrying the generated words; each body at its size, 22
/// issues with a label, every one on this board alone.
pub fn public_board() -> Board {
    let words = carried(&words(), (1, 1));
    let words: Vec<(&str, usize)> = words
        .iter()
        .map(|(word, many)| (word.as_str(), *many))
        .collect();
    let mut board = public_board_of(PUBLIC_TICKETS, PUBLIC_OPEN, &words);
    let issues = board.issues.len();
    board.issues = board
        .issues
        .drain(..)
        .enumerate()
        .map(|(at, issue)| {
            // Each issue's place among the sizes, in an order its position does not set.
            let rank = (at * 1103) % issues;
            let issue = sized(issue, body_size(rank, issues));
            issue.with_labels(usize::from(at % 97 == 0 && at / 97 < LABELLED))
        })
        .collect();
    board
}

/// The private board, project 8 of `example-private-org`: 400 tickets, 95 open, carrying the
/// generated words in proportion.
pub fn private_board() -> Board {
    // The private board's generator scales each count to its size itself.
    let words = carried(&words(), (1, 1));
    let words: Vec<(&str, usize)> = words
        .iter()
        .map(|(word, many)| (word.as_str(), *many))
        .collect();
    private_board_of(PRIVATE_TICKETS, PRIVATE_OPEN, &words)
}

/// A generated draft: `(repository origin, title, paths, body)`.
pub type Draft = (String, String, Vec<String>, String);

/// The `at`th generated draft about `repository`, its title carrying `words`, one or two file
/// names in its paths.
fn generated(at: usize, repository: &str, words: &[&str]) -> Draft {
    let name = repository.rsplit('/').next().unwrap_or_default();
    let title = format!("{name}: {}", words.join(" "));
    let body = format!(
        "When the {} and the {} run together, the {} is lost and the {} keeps going past the {}, so a person sees a stale result.\n",
        words[0], words[1], words[2], words[3], words[4]
    );
    let mut paths = vec![format!("crates/{name}/src/{}", FILES[at % FILES.len()].0)];
    if at.is_multiple_of(2) {
        paths.push(format!("src/{}", FILES[(at + 3) % FILES.len()].0));
    }
    (repository.to_owned(), title, paths, body)
}

/// Record the 50 drafts: the 13 clusters' later members, 27 generated public ones and 10
/// generated private ones; answers each id with whether it is private.
pub fn record_drafts(world: &mut World) -> Vec<(String, bool)> {
    let mut drafts = Vec::new();
    for cluster in clusters() {
        let paths: Vec<&str> = cluster.draft.paths.iter().map(String::as_str).collect();
        let id = world.draft_in(
            BATCH,
            &origin(&cluster.repository),
            &cluster.draft.title,
            &paths,
            &format!("{}\n", cluster.draft.body),
        );
        drafts.push((id, false));
    }
    let words = words();
    // Each public draft five of the generated words, the common, middling and rare ones mixed.
    let order: Vec<&str> = (0..words.len())
        .map(|at| words[(at * 37) % words.len()].0.as_str())
        .collect();
    for at in 0..GENERATED_PUBLIC + GENERATED_PRIVATE {
        let private = at >= GENERATED_PUBLIC;
        let repository = if private {
            private_origin(PRIVATE_REPOSITORIES[at % PRIVATE_REPOSITORIES.len()])
        } else {
            origin(REPOSITORIES[at % REPOSITORIES.len()])
        };
        let start = (at * TERMS) % order.len();
        let terms: Vec<&str> = (0..TERMS)
            .map(|one| order[(start + one) % order.len()])
            .collect();
        let (repository, title, paths, body) = generated(at, &repository, &terms);
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let id = world.draft_in(BATCH, &repository, &title, &paths, &body);
        drafts.push((id, private));
    }
    drafts
}
