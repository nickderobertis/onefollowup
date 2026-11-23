//! A grooming journey's world: a directory holding an onetaskgraph store of local-md sources —
//! `drafts` and the `followups` destination — an `onefollowup.yaml`, one-commit-history git
//! repositories under `repos/` that `https://<host>/<owner>/<name>.git` is pointed at by the
//! world's own git configuration, and a `home/` holding nothing of the developer's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use onefollowup::formats::groom::{GroomRecord, OverlapComment, content_digest, record_key};
use onefollowup::store::{ReadItem, Store, TaskChange};
use serde_json::{Value, json};

/// `path` as a shell script names it: with forward slashes, which a Windows shell reads as
/// separators where it reads a backslash as an escape.
pub fn shell_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// The destination every grooming journey grooms.
pub const DESTINATION: &str = "followups";
/// The namespace every journey's records are under.
pub const NAMESPACE: &str = "orchestrator";

/// What one run of a verb did.
#[derive(Debug, Clone)]
pub struct Ran {
    /// Its exit code.
    pub code: i32,
    /// What it printed on stdout.
    pub stdout: String,
    /// What it printed on stderr.
    pub stderr: String,
}

/// How a suite runs a verb in a world: the built binary, or the library in process.
pub trait Runner {
    /// Run `onefollowup <args>` in `world`'s root with `world`'s environment.
    fn run(&self, world: &World, args: &[&str]) -> Ran;
}

/// One journey's world.
pub struct World {
    _directory: tempfile::TempDir,
    /// The store's directory, where every verb runs.
    pub root: PathBuf,
}

impl World {
    /// An empty world: the store, the configuration, the git configuration, no ticket and no
    /// repository.
    pub fn new() -> Self {
        let base = std::env::temp_dir();
        #[cfg(not(windows))]
        let base = base.canonicalize().expect("the temporary root resolves");
        let directory = tempfile::tempdir_in(base).expect("a temporary directory");
        let root = directory.path().to_path_buf();
        let mut store = String::from("sources:\n");
        for source in ["drafts", DESTINATION] {
            std::fs::create_dir_all(root.join(format!("{source}-root/tasks"))).expect("a root");
            store.push_str(&format!(
                "  {source}:\n    plugin: local-md\n    config:\n      root: {source}-root\n"
            ));
        }
        std::fs::write(root.join("onetaskgraph.yaml"), store).expect("the store");
        std::fs::write(root.join("onefollowup.yaml"), "{}\n").expect("the configuration");
        std::fs::create_dir_all(root.join("home/config")).expect("home");
        std::fs::create_dir_all(root.join("repos")).expect("repos");
        std::fs::create_dir_all(root.join("tmp")).expect("a temporary directory");
        // A `file:` URL with forward slashes: git's configuration reads a backslash in a quoted
        // section name as an escape, so a Windows path written as it displays loses them all.
        let repos = shell_path(&root.join("repos"));
        let repos = repos.strip_prefix('/').unwrap_or(&repos).to_owned();
        std::fs::write(
            root.join("home/.gitconfig"),
            format!(
                "[user]\n\tname = Groom Journey\n\temail = groom@example.invalid\n\
                 [init]\n\tdefaultBranch = main\n\
                 [url \"file:///{repos}/\"]\n\tinsteadOf = https://\n\
                 [uploadpack]\n\tallowFilter = true\n\
                 [protocol \"file\"]\n\tallow = always\n\
                 [advice]\n\tdetachedHead = false\n"
            ),
        )
        .expect("the git configuration");
        Self {
            _directory: directory,
            root,
        }
    }

    /// The environment every verb and every git command runs with: the world's own home, and
    /// a `PATH` that finds git.
    pub fn environment(&self) -> Vec<(String, String)> {
        vec![
            (
                "HOME".to_owned(),
                self.root.join("home").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                self.root.join("home/config").display().to_string(),
            ),
            (
                "PATH".to_owned(),
                crate::isolated::search_path(None, crate::isolated::UNIX_TOOLS),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
            (
                "TMPDIR".to_owned(),
                self.root.join("tmp").display().to_string(),
            ),
        ]
    }

    /// Run `git <args>` in `directory`, require it to succeed, and answer its stdout.
    pub fn git(&self, directory: &Path, args: &[&str]) -> String {
        let output = crate::isolated::command("git", directory, self.environment())
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// The source repository of `repository` (`host/owner/name`).
    pub fn repository_path(&self, repository: &str) -> PathBuf {
        self.root.join("repos").join(format!("{repository}.git"))
    }

    /// Create `repository` with one commit on `main` holding `files`, and answer that commit.
    pub fn repository(&self, repository: &str, files: &[(&str, &str)]) -> String {
        self.repository_on(repository, "main", files)
    }

    /// Create `repository` whose default branch, and only branch, is `branch`, with one commit
    /// on it holding `files`, and answer that commit.
    pub fn repository_on(&self, repository: &str, branch: &str, files: &[(&str, &str)]) -> String {
        let path = self.repository_path(repository);
        std::fs::create_dir_all(&path).expect("the repository's directory");
        self.git(
            &path,
            &["init", "--quiet", &format!("--initial-branch={branch}")],
        );
        self.commit(repository, files, "the first commit")
    }

    /// Commit `files` to `repository`'s checked-out branch — its default branch — and answer
    /// the commit.
    pub fn commit(&self, repository: &str, files: &[(&str, &str)], message: &str) -> String {
        let path = self.repository_path(repository);
        for (file, text) in files {
            let at = path.join(file);
            std::fs::create_dir_all(at.parent().expect("a parent")).expect("a directory");
            std::fs::write(at, text).expect("a file");
        }
        self.git(&path, &["add", "--all"]);
        self.git(
            &path,
            &["commit", "--quiet", "--allow-empty", "--message", message],
        );
        self.git(&path, &["rev-parse", "HEAD"])
    }

    /// Commit `files` to a branch `branch` of `repository` off `main`, never merged, and answer
    /// that commit: one a clone holds that is not on `main`.
    pub fn side_commit(&self, repository: &str, branch: &str, files: &[(&str, &str)]) -> String {
        let path = self.repository_path(repository);
        self.git(&path, &["checkout", "--quiet", "-b", branch]);
        let commit = self.commit(repository, files, "a commit off main");
        self.git(&path, &["checkout", "--quiet", "main"]);
        commit
    }

    /// The path of the destination's ticket `id`.
    pub fn ticket_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{DESTINATION}-root/tasks/{id}.md"))
    }

    /// Write the destination's ticket `id`, as a person or another batch left it.
    pub fn ticket(&self, id: &str, title: &str, status: &str, repositories: &[&str], body: &str) {
        let repositories = if repositories.is_empty() {
            String::new()
        } else {
            format!("repositories: [{}]\n", repositories.join(", "))
        };
        std::fs::write(
            self.ticket_path(id),
            format!("---\ntitle: {title:?}\nstatus: {status}\npriority: low\n{repositories}---\n{body}\n"),
        )
        .expect("a ticket");
    }

    /// The text of the destination's ticket `id`.
    pub fn ticket_text(&self, id: &str) -> String {
        std::fs::read_to_string(self.ticket_path(id)).expect("the ticket is there")
    }

    /// Replace `from` with `to` in the ticket `id`, as a person editing it would.
    pub fn edit_ticket(&self, id: &str, from: &str, to: &str) {
        let text = self.ticket_text(id);
        assert!(text.contains(from), "{id} holds {from:?}: {text}");
        std::fs::write(self.ticket_path(id), text.replacen(from, to, 1)).expect("edited");
    }

    /// Add a comment `comment` to the ticket `id`, created at `created` and last edited at
    /// `updated`, as a person's client left it.
    pub fn comment(&self, id: &str, comment: &str, created: &str, updated: &str, body: &str) {
        let mut text = self.ticket_text(id);
        if !text.contains("\n## Comments\n") {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("\n## Comments\n");
        }
        text.push_str(&format!(
            "\n<!-- onetaskgraph:comment id=\"{comment}\" author=\"person\" created_at=\"{created}\" updated_at=\"{updated}\" -->\n### person — {created}\n\n{body}\n\n<!-- /onetaskgraph:comment -->\n"
        ));
        std::fs::write(self.ticket_path(id), text).expect("the comment is written");
    }

    /// The store, opened as a person's other client would open it.
    pub fn store(&self) -> Store {
        Store::open(Some(&self.root), &self.root, &self.environment()).expect("the store opens")
    }

    /// Edit the comment `comment` of the ticket `id` through the store's comment edit.
    pub fn edit_comment(&self, id: &str, comment: &str, body: &str) {
        self.store()
            .edit_comment(&format!("{DESTINATION}:{id}"), comment, body)
            .expect("the comment is edited");
    }

    /// Write `record` to the ticket `id` through the store, as `groom publish` would.
    pub fn record(&self, id: &str, record: &Value) {
        self.store()
            .update_task(
                &format!("{DESTINATION}:{id}"),
                &TaskChange {
                    metadata: BTreeMap::from([(record_key(NAMESPACE), record.clone())]),
                    ..TaskChange::default()
                },
            )
            .expect("the record is written");
    }

    /// The `groom` record of the ticket `id` as it stands now, verified at `verified_at`
    /// against `repository` at `commit`, its verdict resting on `paths`, holding `overlap`.
    pub fn groomed(
        &self,
        id: &str,
        verified_at: &str,
        repository: &str,
        commit: &str,
        paths: &[&str],
        overlap: &[(&str, &str)],
    ) -> GroomRecord {
        let read = self
            .store()
            .read(&[format!("{DESTINATION}:{id}")], false)
            .expect("the ticket reads");
        let item = read[0].item.clone().expect("the ticket is there");
        GroomRecord {
            schema_version: 1,
            batch: "groom-20261101T000000000Z".to_owned(),
            verified_at: verified_at.to_owned(),
            verdict: onefollowup::formats::groom::GroomVerdict::Keep,
            repository: repository.to_owned(),
            commit: commit.to_owned(),
            evidence_paths: paths.iter().map(|&path| path.to_owned()).collect(),
            content_digest: content_digest(&item.title, &item.content),
            overlap_comments: overlap
                .iter()
                .map(|(id, edited_at)| OverlapComment {
                    id: (*id).to_owned(),
                    edited_at: (*edited_at).to_owned(),
                })
                .collect(),
        }
    }

    /// The grooming batch `batch`'s directory.
    pub fn batch(&self, batch: &str) -> PathBuf {
        self.root.join(".onefollowup").join("groom").join(batch)
    }

    /// The batch record `batch` holds.
    pub fn batch_record(&self, batch: &str) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.batch(batch).join("batch.json")).expect("batch.json"),
        )
        .expect("JSON")
    }

    /// The tickets `batch` selected, by id.
    pub fn selected(&self, batch: &str) -> Vec<String> {
        self.batch_record(batch)["selected"]
            .as_array()
            .expect("selected")
            .iter()
            .map(|one| one["ticket"].as_str().expect("an id").to_owned())
            .collect()
    }

    /// Run `args`, require it to exit `code`, and answer the JSON it printed.
    pub fn json(&self, runner: &dyn Runner, code: i32, args: &[&str]) -> Value {
        let ran = runner.run(self, args);
        assert_eq!(
            ran.code,
            code,
            "`onefollowup {}`: {}{}",
            args.join(" "),
            ran.stderr,
            ran.stdout
        );
        serde_json::from_str(&ran.stdout).unwrap_or_else(|error| {
            panic!(
                "`onefollowup {}` printed no JSON ({error}): {}",
                args.join(" "),
                ran.stdout
            )
        })
    }
}

impl World {
    /// The qualified id of the destination's ticket `id`.
    pub fn qualified(id: &str) -> String {
        format!("{DESTINATION}:{id}")
    }

    /// The ticket `id` and its comments as the store reads them now.
    pub fn read(&self, id: &str) -> ReadItem {
        self.store()
            .read(&[Self::qualified(id)], true)
            .expect("the ticket reads")
            .remove(0)
    }

    /// The ticket `id`'s status category as the store reads it now.
    pub fn status(&self, id: &str) -> String {
        self.read(id)
            .item
            .expect("the ticket is there")
            .status
            .category
    }

    /// The ticket `id`'s comments' bodies as the store reads them now.
    pub fn comments(&self, id: &str) -> Vec<String> {
        self.read(id)
            .comments
            .expect("its comments read")
            .into_iter()
            .map(|comment| comment.body)
            .collect()
    }

    /// Remove the comment `comment` from the ticket `id`, as a person deleting it would.
    pub fn remove_comment(&self, id: &str, comment: &str) {
        let text = self.ticket_text(id);
        let open = format!("<!-- onetaskgraph:comment id=\"{comment}\"");
        let start = text.find(&open).expect("the comment is there");
        let close = "<!-- /onetaskgraph:comment -->\n";
        let end = start + text[start..].find(close).expect("the comment ends") + close.len();
        std::fs::write(
            self.ticket_path(id),
            format!("{}{}", &text[..start], &text[end..]),
        )
        .expect("the comment is removed");
    }

    /// Move the ticket `id` to the status category `status` through the store, as a person's
    /// client or another process would.
    pub fn move_to(&self, id: &str, status: &str) {
        self.store()
            .update_task(
                &Self::qualified(id),
                &TaskChange {
                    status: Some(status.to_owned()),
                    ..TaskChange::default()
                },
            )
            .expect("the status is written");
    }

    /// The ticket `id`'s `groom` record as the store reads it now.
    pub fn groom_record(&self, id: &str) -> Option<Value> {
        self.read(id)
            .item
            .expect("the ticket is there")
            .metadata
            .get(&record_key(NAMESPACE))
            .cloned()
    }

    /// Play the verifying agents of `batch`: write each shard's `verdicts.jsonl`, one record
    /// per ticket, from `verdict`.
    pub fn verdicts(&self, batch: &str, verdict: impl Fn(&str) -> Value) {
        for shard in self.batch_record(batch)["shards"]
            .as_array()
            .expect("shards")
        {
            let mut text = String::new();
            for ticket in shard["tickets"].as_array().expect("tickets") {
                let id = ticket.as_str().expect("an id");
                text.push_str(&verdict(id.trim_start_matches("followups:")).to_string());
                text.push('\n');
            }
            std::fs::write(shard["verdicts"].as_str().expect("a path"), text)
                .expect("verdicts written");
        }
    }

    /// One full pass: prepare, the agents' `verdict`s, a passing check; answers the batch, or
    /// `None` when prepare selected nothing. Publish is the caller's.
    pub fn checked(&self, runner: &dyn Runner, verdict: impl Fn(&str) -> Value) -> Option<String> {
        let prepared = self.json(runner, 0, &["groom", "prepare"]);
        if prepared["shards"] == json!([]) {
            return None;
        }
        let batch = prepared["batch"].as_str().expect("a batch").to_owned();
        self.verdicts(&batch, verdict);
        let checked = self.json(runner, 0, &["groom", "check", "--batch", &batch]);
        assert_eq!(checked["problems"], json!([]), "{checked:#}");
        Some(batch)
    }

    /// One full pass, published: answers the batch and what publish printed.
    pub fn groomed_pass(
        &self,
        runner: &dyn Runner,
        verdict: impl Fn(&str) -> Value,
    ) -> (String, Value) {
        let batch = self
            .checked(runner, verdict)
            .expect("prepare selected tickets");
        let published = self.json(runner, 0, &["groom", "publish", "--batch", &batch]);
        (batch, published)
    }
}

/// A verdict record on the ticket `id`.
pub fn verdict(
    id: &str,
    verdict: &str,
    confidence: &str,
    commit: Option<&str>,
    paths: &[&str],
    duplicate_of: Option<&str>,
) -> Value {
    json!({
        "schema_version": 1,
        "ticket": World::qualified(id),
        "verdict": verdict,
        "confidence": confidence,
        "evidence": format!("{id}: read what {} rests on at main", if paths.is_empty() { "the ticket".to_owned() } else { paths.join(", ") }),
        "deciding_commit": commit,
        "evidence_paths": paths,
        "duplicate_of": duplicate_of.map(World::qualified),
        "root_cause": duplicate_of.map(|_| "the layout's save reads the window size twice"),
        "seam_repository": null,
    })
}

/// A `keep` verdict on `id`, resting on `paths`.
pub fn keep(id: &str, paths: &[&str]) -> Value {
    verdict(id, "keep", "high", None, paths, None)
}

/// The marker line a grooming comment of `batch` with `mark` ends in.
pub fn grooming_marker(batch: &str, mark: &str) -> String {
    format!(
        "<!-- {NAMESPACE}:follow-up-comment run=\"{batch}\" kind=\"groom\" verdict=\"{mark}\" -->"
    )
}

/// `now` less `seconds`, as RFC 3339 to the second.
pub fn ago(seconds: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
