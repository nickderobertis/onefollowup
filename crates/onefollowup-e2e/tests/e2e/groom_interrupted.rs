//! `groom publish` stopped part way through its writes, then run again in a fresh process.
//!
//! The stop is a real one: the journey holds one ticket's local-md lock — the lock beside its
//! file that every version-held write of it waits for — so publish's `update-many` lands the
//! closes before that ticket and blocks on it, and the journey kills the process there, before
//! any answer is receipted and before any comment is sent. Another process then moves tickets
//! through the store, and the retry reads the receipts: it re-reads every close no receipt
//! confirms, confirms the closes that landed by the record they hold, leaves what a person
//! claimed, and posts each comment once.

use std::fs::File;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::groom::world::{World, grooming_marker, keep, verdict};
use crate::grooming::Binary;

const WIDGETS: &str = "github.com/example/widgets";

/// Hold the local-md lock of the ticket `id`, as another writer of it would.
fn hold(world: &World, id: &str) -> File {
    let path = world.ticket_path(id).with_extension("md.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .expect("the lock file opens");
    file.lock().expect("the lock is taken");
    file
}

/// Start `groom publish --batch <batch>` as a process of its own.
fn spawn_publish(world: &World, batch: &str) -> std::process::Child {
    crate::isolated::command(crate::isolated::binary(), &world.root, world.environment())
        .args(["groom", "publish", "--batch", batch])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("publish starts")
}

/// Wait until the ticket `id`'s file reads at `status`.
fn wait_for(world: &World, id: &str, status: &str) {
    let deadline = Instant::now() + Duration::from_secs(50);
    while !world
        .ticket_text(id)
        .contains(&format!("\nstatus: {status}\n"))
    {
        assert!(
            Instant::now() < deadline,
            "{id} never reached {status}: {}",
            world.ticket_text(id)
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// No ticket of `ids` reads open while carrying a grooming comment saying it was closed.
fn no_open_ticket_says_it_closed(world: &World, batch: &str, ids: &[&str]) {
    for id in ids {
        let open = !["done", "cancelled"].contains(&world.status(id).as_str());
        let says_closed = world.comments(id).iter().any(|body| {
            ["fixed", "obsolete", "duplicate"]
                .iter()
                .any(|mark| body.contains(&grooming_marker(batch, mark)))
        });
        assert!(
            !(open && says_closed),
            "{id} is open and carries a comment saying it was closed"
        );
    }
}

fn left_for_person(published: &Value) -> Value {
    serde_json::from_str::<Value>(
        &std::fs::read_to_string(published["report_json"].as_str().expect("a path"))
            .expect("report.json"),
    )
    .expect("JSON")["left_for_person"]
        .clone()
}

#[test]
fn a_retry_comments_each_landed_close_once_and_leaves_a_promoted_duplicate() {
    let world = World::new();
    world.repository(WIDGETS, &[("src/a.rs", "a\n"), ("src/b.rs", "b\n")]);
    let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix the save");
    for id in ["a1", "d1", "s1", "x1"] {
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            "backlog",
            &[WIDGETS],
            &format!("The body of {id}."),
        );
    }
    let batch = world
        .checked(&Binary, |id| match id {
            "a1" => verdict(id, "fixed", "high", Some(&fix), &["src/a.rs"], None),
            // `s1` rests on the most evidence, so it survives the cluster.
            "s1" => keep(id, &["src/a.rs", "src/b.rs"]),
            _ => verdict(id, "keep", "high", None, &["src/b.rs"], Some("s1")),
        })
        .expect("a batch");

    // Publish closes `a1` and folds `d1`, then waits on `x1`'s lock; it is stopped there.
    let lock = hold(&world, "x1");
    let mut publish = spawn_publish(&world, &batch);
    wait_for(&world, "d1", "cancelled");
    publish.kill().expect("publish is stopped");
    publish.wait().expect("publish ended");
    drop(lock);
    let all = ["a1", "d1", "s1", "x1"];
    assert_eq!(world.status("a1"), "done");
    assert_eq!(world.status("x1"), "backlog");
    for id in all {
        assert!(world.comments(id).is_empty(), "{id}: no comment was sent");
    }
    no_open_ticket_says_it_closed(&world, &batch, &all);

    // Another process promotes `x1` to Todo; the retry, in a fresh process, leaves it.
    world.move_to("x1", "todo");
    let promoted = world.ticket_text("x1");
    let published = world.json(&Binary, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(published["mismatched"], json!([]), "{published:#}");
    assert_eq!(published["confirmed"], 3, "a1, d1 and s1: {published:#}");

    for (id, mark) in [("a1", "fixed"), ("d1", "duplicate")] {
        let comments = world.comments(id);
        assert_eq!(comments.len(), 1, "{id}'s comment is posted exactly once");
        assert!(comments[0].contains(&grooming_marker(&batch, mark)));
    }
    let folded = world.read("d1").item.expect("d1");
    assert_eq!(folded.duplicate_of().as_deref(), Some("followups:s1"));
    assert_eq!(world.ticket_text("x1"), promoted, "x1 is left as it was");
    assert_eq!(world.status("x1"), "todo");
    assert!(world.comments("x1").is_empty());
    assert!(
        world.groom_record("x1").is_none(),
        "its groom key unchanged"
    );
    let x1 = world.read("x1").item.expect("x1");
    assert_eq!(x1.duplicate_of(), None, "no duplicate link");

    let survivor = world.comments("s1");
    assert_eq!(survivor.len(), 1, "one consolidation comment: {survivor:?}");
    assert!(survivor[0].contains("followups:d1"), "{}", survivor[0]);
    assert!(
        !survivor[0].contains("followups:x1"),
        "it names only duplicates whose closes landed: {}",
        survivor[0]
    );
    let left = left_for_person(&published);
    assert_eq!(left.as_array().map(Vec::len), Some(1), "{left:#}");
    assert_eq!(left[0]["ticket"], "followups:x1");
    assert_eq!(left[0]["survivor"], "followups:s1");
    assert_eq!(left[0]["status"]["category"], "todo");
    let markdown =
        std::fs::read_to_string(published["report"].as_str().expect("a path")).expect("report.md");
    assert!(markdown.contains("`followups:x1`"), "{markdown}");
    no_open_ticket_says_it_closed(&world, &batch, &all);
}

#[test]
fn a_retry_leaves_what_was_claimed_after_the_stop_and_sends_the_rest() {
    let world = World::new();
    world.repository(WIDGETS, &[("src/a.rs", "a\n"), ("src/b.rs", "b\n")]);
    let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix the save");
    for (id, status) in [
        ("a2", "backlog"),
        ("b2", "backlog"),
        ("k2", "backlog"),
        ("s2", "backlog"),
        ("t2", "todo"),
    ] {
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            status,
            &[WIDGETS],
            &format!("The body of {id}."),
        );
    }
    let batch = world
        .checked(&Binary, |id| match id {
            "a2" | "t2" => verdict(id, "fixed", "high", Some(&fix), &["src/a.rs"], None),
            "b2" => verdict(id, "keep", "high", None, &["src/b.rs"], Some("s2")),
            "s2" => keep(id, &["src/a.rs", "src/b.rs"]),
            _ => keep(id, &["src/b.rs"]),
        })
        .expect("a batch");
    let plan: Value = serde_json::from_str(
        &std::fs::read_to_string(world.batch(&batch).join("plan.json")).expect("plan.json"),
    )
    .expect("JSON");
    assert!(
        plan["plan"]["fixed"]
            .as_array()
            .expect("fixed")
            .iter()
            .any(|one| one["ticket"] == "followups:t2"),
        "the definitely-outdated Todo ticket is planned closed: {plan:#}"
    );

    // Publish closes `a2`, then waits on `t2`'s lock; it is stopped there.
    let lock = hold(&world, "t2");
    let mut publish = spawn_publish(&world, &batch);
    wait_for(&world, "a2", "done");
    publish.kill().expect("publish is stopped");
    publish.wait().expect("publish ended");
    drop(lock);
    assert!(
        world.groom_record("k2").is_none(),
        "k2's record was not sent"
    );

    // Another process claims `t2` and promotes `b2` between the stop and the retry.
    world.move_to("t2", "in-progress");
    world.move_to("b2", "todo");
    let claimed = world.ticket_text("t2");
    let promoted = world.ticket_text("b2");
    let published = world.json(&Binary, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(published["mismatched"], json!([]), "{published:#}");

    assert_eq!(world.ticket_text("t2"), claimed, "t2 is left as it was");
    assert_eq!(world.ticket_text("b2"), promoted, "b2 is left as it was");
    assert_eq!(world.status("t2"), "in-progress");
    assert_eq!(world.status("b2"), "todo");
    let comments = world.comments("a2");
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert!(comments[0].contains(&grooming_marker(&batch, "fixed")));
    assert_eq!(
        world.groom_record("k2").expect("k2's record")["batch"],
        batch.as_str(),
        "the retry still sends the writes no receipt confirms"
    );
    assert!(
        world.comments("s2").is_empty(),
        "a survivor with no duplicate closed gets no comment"
    );
    let left = left_for_person(&published);
    let mut ids: Vec<&str> = left
        .as_array()
        .expect("a list")
        .iter()
        .map(|one| one["ticket"].as_str().expect("an id"))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["followups:b2", "followups:t2"], "{left:#}");
}
