//! Incremental selection over tickets holding `groom` records written through the store with
//! onefollowup's own record type: nothing changed selects nothing; each change, made alone,
//! selects exactly the ticket it touched; `--full` selects every open ticket; a comment inside
//! the overlap selects by what the record holds of it; and an older record holds the comment
//! read back so an edit before every newer record is still read.

use serde_json::json;

use super::world::{Runner, World, ago};

const WIDGETS: &str = "github.com/example/widgets";
const GADGETS: &str = "github.com/example/gadgets";

/// A world whose every open ticket holds a current record, and the commits it was verified at.
struct Groomed {
    world: World,
    widgets: String,
    gadgets: String,
    /// When every record was verified.
    verified_at: String,
}

/// Each ticket: its id, its repository and the evidence paths its record names.
const TICKETS: [(&str, &str, &[&str]); 6] = [
    ("wa", WIDGETS, &["src/a.rs"]),
    ("wb", WIDGETS, &["src/b.rs"]),
    ("wc", WIDGETS, &["src/b.rs"]),
    ("ga", GADGETS, &[]),
    ("gb", GADGETS, &["lib/y.rs"]),
    ("ov", WIDGETS, &["docs/guide.md"]),
];

impl Groomed {
    /// Every ticket of [`TICKETS`], verified ten minutes ago: `wc` with a comment older than the
    /// overlap, `ov` with one inside it that its record holds unchanged; and `old`, closed.
    fn new() -> Self {
        Self::verified(&ago(600), |_| None)
    }

    /// As [`Self::new`], each ticket's record verified at `verified_at` unless `at` names
    /// another time for it.
    fn verified(verified_at: &str, at: impl Fn(&str) -> Option<String>) -> Self {
        let world = World::new();
        let widgets = world.repository(
            WIDGETS,
            &[
                ("src/a.rs", "a\n"),
                ("src/b.rs", "b\n"),
                ("docs/guide.md", "guide\n"),
            ],
        );
        let gadgets = world.repository(GADGETS, &[("lib/x.rs", "x\n"), ("lib/y.rs", "y\n")]);
        for (id, repository, _) in TICKETS {
            world.ticket(
                id,
                &format!("widgets: ticket {id}"),
                "backlog",
                &[repository],
                &format!("The body of {id}, about what it names."),
            );
        }
        world.ticket(
            "old",
            "widgets: a closed ticket",
            "done",
            &[WIDGETS],
            "Done.",
        );
        world.comment("wc", "c-wc", &ago(7_200), &ago(7_200), "An older question.");
        let inside = ago(900);
        world.comment(
            "ov",
            "c-ov",
            &inside,
            &inside,
            "A question inside the overlap.",
        );
        for (id, repository, paths) in TICKETS {
            let commit = if repository == WIDGETS {
                &widgets
            } else {
                &gadgets
            };
            let overlap: &[(&str, &str)] = if id == "ov" {
                &[("c-ov", &inside)]
            } else {
                &[]
            };
            let when = at(id).unwrap_or_else(|| verified_at.to_owned());
            let record = world.groomed(id, &when, repository, commit, paths, overlap);
            world.record(id, &record.to_value());
        }
        Self {
            world,
            widgets,
            gadgets,
            verified_at: verified_at.to_owned(),
        }
    }

    /// Run `groom prepare` and answer the tickets it selected.
    fn selects(&self, runner: &dyn Runner, args: &[&str]) -> Vec<String> {
        let mut all = vec!["groom", "prepare"];
        all.extend_from_slice(args);
        let printed = self.world.json(runner, 0, &all);
        if printed["shards"] == json!([]) {
            assert_eq!(printed["selected"], 0);
            return Vec::new();
        }
        let selected = self
            .world
            .selected(printed["batch"].as_str().expect("a batch"));
        assert_eq!(printed["selected"], selected.len());
        selected
            .into_iter()
            .map(|id| id.trim_start_matches("followups:").to_owned())
            .collect()
    }
}

pub fn nothing_changed_selects_nothing(runner: &dyn Runner) {
    let groomed = Groomed::new();
    let printed = groomed.world.json(runner, 0, &["groom", "prepare"]);
    assert_eq!(printed["selected"], 0, "{printed:#}");
    assert_eq!(printed["unchanged"], 6);
    assert_eq!(printed["shards"], json!([]));
    let operations: Vec<&str> = printed["store_operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|one| one["op"].as_str().expect("an op"))
        .collect();
    assert_eq!(operations, ["list", "comments-since"], "no ticket to read");
}

pub fn each_change_alone_selects_exactly_its_ticket(runner: &dyn Runner) {
    let marker = "<!-- orchestrator:follow-up-comment run=\"groom-20261101T000000000Z\" root_cause=\"groom\" kind=\"groom\" -->";
    type Change = Box<dyn Fn(&Groomed)>;
    let changes: Vec<(&str, Change, &[&str])> = vec![
        (
            "a ticket newly added",
            Box::new(|g: &Groomed| {
                g.world
                    .ticket("wn", "widgets: a new ticket", "backlog", &[WIDGETS], "New.");
            }),
            &["wn"],
        ),
        (
            "a changed title",
            Box::new(|g: &Groomed| {
                g.world
                    .edit_ticket("wa", "widgets: ticket wa", "widgets: ticket wa, retitled");
            }),
            &["wa"],
        ),
        (
            "a changed body, its metadata block as it was",
            Box::new(|g: &Groomed| {
                g.world
                    .edit_ticket("wb", "The body of wb", "The edited body of wb");
            }),
            &["wb"],
        ),
        (
            "an existing comment edited through the store",
            Box::new(|g: &Groomed| {
                g.world
                    .edit_comment("wc", "c-wc", "An older question, edited.");
            }),
            &["wc"],
        ),
        (
            "a person's new comment",
            Box::new(|g: &Groomed| {
                let now = ago(0);
                g.world
                    .comment("ga", "c-new", &now, &now, "Is this still a problem?");
            }),
            &["ga"],
        ),
        (
            "a commit touching the ticket's evidence path",
            Box::new(|g: &Groomed| {
                g.world
                    .commit(WIDGETS, &[("src/a.rs", "a changed\n")], "change a");
            }),
            &["wa"],
        ),
        (
            "any move of the repository of a ticket with no paths",
            Box::new(|g: &Groomed| {
                g.world
                    .commit(GADGETS, &[("lib/x.rs", "x changed\n")], "change x");
            }),
            &["ga"],
        ),
        (
            "a record older than max_age_days",
            Box::new(|g: &Groomed| {
                let record = g.world.groomed(
                    "gb",
                    &ago(31 * 86_400),
                    GADGETS,
                    &g.gadgets,
                    &["lib/y.rs"],
                    &[],
                );
                g.world.record("gb", &record.to_value());
            }),
            &["gb"],
        ),
        (
            "a record of an unknown schema",
            Box::new(|g: &Groomed| {
                let mut record = g
                    .world
                    .groomed(
                        "ov",
                        &g.verified_at,
                        WIDGETS,
                        &g.widgets,
                        &["docs/guide.md"],
                        &[],
                    )
                    .to_value();
                record["schema_version"] = json!(2);
                g.world.record("ov", &record);
            }),
            &["ov"],
        ),
        (
            "a ticket moved to another repository",
            Box::new(|g: &Groomed| {
                g.world.edit_ticket(
                    "wa",
                    &format!("repositories: [{WIDGETS}]"),
                    &format!("repositories: [{GADGETS}]"),
                );
            }),
            &["wa"],
        ),
        (
            "a record whose commit main no longer holds",
            Box::new(|g: &Groomed| {
                let gone = "1111111111111111111111111111111111111111";
                let record =
                    g.world
                        .groomed("wb", &g.verified_at, WIDGETS, gone, &["src/b.rs"], &[]);
                g.world.record("wb", &record.to_value());
            }),
            &["wb"],
        ),
        (
            "a record this does not read",
            Box::new(|g: &Groomed| {
                let mut record = g
                    .world
                    .groomed(
                        "gb",
                        &g.verified_at,
                        GADGETS,
                        &g.gadgets,
                        &["lib/y.rs"],
                        &[],
                    )
                    .to_value();
                record["commit"] = json!("--output=/tmp/elsewhere");
                g.world.record("gb", &record);
            }),
            &["gb"],
        ),
        (
            "a move touching none of a ticket's paths",
            Box::new(|g: &Groomed| {
                g.world
                    .commit(WIDGETS, &[("README.md", "readme\n")], "add a readme");
            }),
            &[],
        ),
        (
            "a grooming comment its only new comment",
            Box::new(move |g: &Groomed| {
                let now = ago(0);
                g.world.comment(
                    "wb",
                    "c-groom",
                    &now,
                    &now,
                    &format!("Verified at main.\n\n{marker}"),
                );
            }),
            &[],
        ),
    ];
    for (case, change, expected) in changes {
        let groomed = Groomed::new();
        assert_eq!(
            groomed.selects(runner, &[]),
            Vec::<String>::new(),
            "{case}: before it"
        );
        change(&groomed);
        assert_eq!(
            groomed.selects(runner, &[]),
            expected
                .iter()
                .map(|&one| one.to_owned())
                .collect::<Vec<_>>(),
            "{case} selects exactly the ticket it touched"
        );
    }
}

pub fn full_selects_every_open_ticket(runner: &dyn Runner) {
    let groomed = Groomed::new();
    let mut expected: Vec<String> = TICKETS.iter().map(|(id, _, _)| (*id).to_owned()).collect();
    expected.sort();
    assert_eq!(
        groomed.selects(runner, &[]),
        Vec::<String>::new(),
        "without it, nothing"
    );
    assert_eq!(groomed.selects(runner, &["--full"]), expected);
}

pub fn overlap_comments_select_by_what_the_record_holds(runner: &dyn Runner) {
    // A comment inside the overlap the record does not hold.
    let groomed = Groomed::new();
    let inside = ago(1_200);
    groomed
        .world
        .comment("wa", "c-late", &inside, &inside, "Indexed late.");
    assert_eq!(groomed.selects(runner, &[]), ["wa"]);

    // One the record holds with another last-edited time.
    let groomed = Groomed::new();
    let inside = ago(1_500);
    let edited = ago(1_000);
    groomed
        .world
        .comment("wb", "c-wb", &inside, &edited, "Edited inside the overlap.");
    let record = groomed.world.groomed(
        "wb",
        &groomed.verified_at,
        WIDGETS,
        &groomed.widgets,
        &["src/b.rs"],
        &[("c-wb", &inside)],
    );
    groomed.world.record("wb", &record.to_value());
    assert_eq!(groomed.selects(runner, &[]), ["wb"]);

    // And the same comment, held as it stands, selects nothing: `ov`'s in every world.
    let record = groomed.world.groomed(
        "wb",
        &groomed.verified_at,
        WIDGETS,
        &groomed.widgets,
        &["src/b.rs"],
        &[("c-wb", &edited)],
    );
    groomed.world.record("wb", &record.to_value());
    assert_eq!(groomed.selects(runner, &[]), Vec::<String>::new());
}

pub fn an_older_record_holds_the_comment_read_back(runner: &dyn Runner) {
    // `wb`'s record is three days old; every other ticket's is ten minutes old. A comment on
    // `wb` made four days ago and edited two days ago is older than every newer record, and
    // only a read starting at the earliest `verified_at` less the overlap lists it.
    let groomed = Groomed::verified(&ago(600), |id| (id == "wb").then(|| ago(3 * 86_400)));
    assert_eq!(
        groomed.selects(runner, &[]),
        Vec::<String>::new(),
        "before the edit"
    );
    groomed.world.comment(
        "wb",
        "c-edit",
        &ago(4 * 86_400),
        &ago(2 * 86_400),
        "Edited two days ago.",
    );
    assert_eq!(groomed.selects(runner, &[]), ["wb"]);
}

pub fn a_record_this_does_not_read_selects_its_ticket(runner: &dyn Runner) {
    let groomed = Groomed::new();
    let good = groomed
        .world
        .groomed(
            "gb",
            &groomed.verified_at,
            GADGETS,
            &groomed.gadgets,
            &["lib/y.rs"],
            &[],
        )
        .to_value();
    let unreadable: [(&str, serde_json::Value); 6] = [
        ("repository", json!("gadgets")),
        ("verified_at", json!("yesterday")),
        ("evidence_paths", json!(["/etc/passwd"])),
        ("content_digest", json!("md5:0")),
        (
            "overlap_comments",
            json!([{"id": "c-1", "edited_at": "soon"}]),
        ),
        ("verdict", json!("maybe")),
    ];
    for (key, value) in unreadable {
        let mut record = good.clone();
        record[key] = value;
        groomed.world.record("gb", &record);
        assert_eq!(
            groomed.selects(runner, &[]),
            ["gb"],
            "a record whose `{key}` this does not read"
        );
        let reasons =
            &groomed.world.batch_record(&latest(&groomed.world))["selected"][0]["reasons"];
        assert!(
            reasons[0]
                .as_str()
                .is_some_and(|reason| reason.starts_with("its groom record is not one this reads")),
            "{key}: {reasons}"
        );
    }
    groomed.world.record("gb", &good);
    assert_eq!(
        groomed.selects(runner, &[]),
        Vec::<String>::new(),
        "a readable record again"
    );
}

/// The newest batch the world holds.
fn latest(world: &World) -> String {
    let mut batches: Vec<String> = std::fs::read_dir(world.root.join(".onefollowup/groom"))
        .expect("the grooming directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("groom-"))
        .collect();
    batches.sort();
    batches.pop().expect("a batch")
}
