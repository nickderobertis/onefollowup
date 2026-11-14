//! One journey's world: a directory holding an onetaskgraph store of local-md sources — the
//! `drafts` source and each destination — an `onefollowup.yaml`, a `bin/` of commands the
//! configuration names on `PATH`, and the built binary run there with nothing of this process's
//! environment. Every verb's printed `store_operations` is collected by phase, so a journey
//! asserts what onefollowup asked of each source, counted at onefollowup's own store boundary.

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use serde_json::{Value, json};

/// The host label every journey's configuration states.
pub const HOST: &str = "build-host-1";
/// The basis commit every journey's tickets record.
pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
/// The repository every journey's drafts are about.
pub const REPOSITORY: &str = "github.com/example/widgets";

/// One journey's world.
pub struct World {
    _directory: tempfile::TempDir,
    /// The store's directory, where every verb runs.
    pub root: PathBuf,
    /// The phases run so far, each with the store operations it printed.
    pub phases: Vec<(String, Vec<Value>)>,
    /// Variables every verb's environment holds beyond the world's own.
    pub variables: Vec<(String, String)>,
}

impl World {
    /// A world whose store holds `drafts` and each of `destinations`, configured by
    /// `onefollowup.yaml`.
    pub fn new(destinations: &[&str], config: &str) -> Self {
        Self::assigning(destinations, &[], config)
    }

    /// A world whose store holds `drafts` and each of `destinations`, each source `assigning`
    /// names holding assignees (`config.assignees: true`), configured by `onefollowup.yaml`.
    pub fn assigning(destinations: &[&str], assigning: &[&str], config: &str) -> Self {
        let root = std::env::temp_dir();
        #[cfg(not(windows))]
        let root = root.canonicalize().expect("the temporary root resolves");
        let directory = tempfile::tempdir_in(root).expect("a temporary directory");
        let root = directory.path().to_path_buf();
        let mut store = String::from("sources:\n");
        for source in std::iter::once("drafts").chain(destinations.iter().copied()) {
            std::fs::create_dir_all(root.join(format!("{source}-root/tasks"))).expect("a root");
            store.push_str(&format!(
                "  {source}:\n    plugin: local-md\n    config:\n      root: {source}-root\n"
            ));
            if assigning.contains(&source) {
                store.push_str("      assignees: true\n");
            }
        }
        std::fs::write(root.join("onetaskgraph.yaml"), store).expect("the store");
        std::fs::write(root.join("onefollowup.yaml"), config).expect("the configuration");
        std::fs::create_dir_all(root.join("bin")).expect("bin");
        std::fs::create_dir_all(root.join("home")).expect("home");
        Self {
            _directory: directory,
            root,
            phases: Vec::new(),
            variables: Vec::new(),
        }
    }

    /// Seed `count` unrelated items into `destination`.
    pub fn seed(&self, destination: &str, count: usize) {
        for at in 0..count {
            self.item(
                destination,
                &format!("other-{at:03}"),
                &format!("title: \"gizmos: unrelated item {at}\"\nstatus: backlog\npriority: low"),
                &format!("An unrelated item, number {at}, about gizmo calibration.\n"),
            );
        }
    }

    /// Write the item `id` of `destination` as a person or another batch left it.
    pub fn item(&self, destination: &str, id: &str, front: &str, body: &str) {
        std::fs::write(
            self.root.join(format!("{destination}-root/tasks/{id}.md")),
            format!("---\n{front}\n---\n{body}"),
        )
        .expect("an item");
    }

    /// The text of `destination`'s item `id`.
    pub fn read_item(&self, destination: &str, id: &str) -> String {
        std::fs::read_to_string(self.root.join(format!("{destination}-root/tasks/{id}.md")))
            .expect("the item is there")
    }

    /// Replace `from` with `to` in `destination`'s item `id`, as a person editing it would.
    pub fn edit_item(&self, destination: &str, id: &str, from: &str, to: &str) {
        let path = self.root.join(format!("{destination}-root/tasks/{id}.md"));
        let text = std::fs::read_to_string(&path).expect("the item is there");
        assert!(text.contains(from), "{id} holds {from:?}: {text}");
        std::fs::write(&path, text.replacen(from, to, 1)).expect("edited");
    }

    /// Every item file of `destination`, by id.
    pub fn items(&self, destination: &str) -> Vec<(String, String)> {
        let mut found: Vec<(String, String)> =
            std::fs::read_dir(self.root.join(format!("{destination}-root/tasks")))
                .expect("the tasks directory")
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
                .map(|path| {
                    (
                        path.file_stem()
                            .expect("a stem")
                            .to_string_lossy()
                            .into_owned(),
                        std::fs::read_to_string(&path).expect("readable"),
                    )
                })
                .collect();
        found.sort();
        found
    }

    /// The items of `destination` this journey did not seed.
    pub fn filed(&self, destination: &str) -> Vec<(String, String)> {
        self.items(destination)
            .into_iter()
            .filter(|(id, _)| !id.starts_with("other-"))
            .collect()
    }

    /// Write an executable shell script `name` on the world's `PATH`.
    pub fn script(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
        }
    }

    /// The built binary, in `directory`, with an environment of the world's own.
    pub fn command_in(&self, directory: &Path) -> Command {
        let mut command = Command::cargo_bin("onefollowup").expect("the binary is built");
        command
            .current_dir(directory)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home/config"))
            .env("XDG_CACHE_HOME", self.root.join("home/cache"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .envs(self.variables.iter().map(|(name, value)| (name, value)));
        command
    }

    /// Run `args` in the world's root with `stdin`.
    pub fn run(&self, args: &[&str], stdin: &str) -> Output {
        self.command_in(&self.root)
            .args(args)
            .write_stdin(stdin)
            .output()
            .expect("the binary runs")
    }

    /// Run `args` as phase `phase`, require it to succeed, and keep its store operations.
    pub fn ok(&mut self, phase: &str, args: &[&str], stdin: &str) -> Value {
        let output = self.run(args, stdin);
        assert!(
            output.status.success(),
            "{phase}: `onefollowup {}` exited {:?}: {}{}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let printed: Value = serde_json::from_slice(&output.stdout).expect("the verb printed JSON");
        self.keep(phase, &printed);
        printed
    }

    /// Keep `printed`'s store operations under `phase`.
    pub fn keep(&mut self, phase: &str, printed: &Value) {
        let operations = printed["store_operations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.phases.push((phase.to_owned(), operations));
    }

    /// Run `args`, require it to exit `code`, and answer its stderr.
    pub fn fails(&self, code: i32, args: &[&str], stdin: &str) -> String {
        let output = self.run(args, stdin);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(
            output.status.code(),
            Some(code),
            "`onefollowup {}`: {stderr}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout)
        );
        stderr
    }

    /// The destination operations of every phase kept so far, as `(phase, op, source, items)`.
    pub fn destination_operations(&self) -> Vec<(String, String, String, u64)> {
        self.phases
            .iter()
            .flat_map(|(phase, operations)| {
                operations
                    .iter()
                    .filter(|operation| operation["destination"] == true)
                    .map(move |operation| {
                        (
                            phase.clone(),
                            operation["op"].as_str().unwrap_or_default().to_owned(),
                            operation["source"].as_str().unwrap_or_default().to_owned(),
                            operation["items"].as_u64().unwrap_or_default(),
                        )
                    })
            })
            .collect()
    }

    /// Forget the phases kept so far.
    pub fn reset_phases(&mut self) {
        self.phases.clear();
    }

    /// The work root.
    pub fn work(&self) -> PathBuf {
        self.root.join(".onefollowup")
    }

    /// The batch's receipt journal, one JSON value per line.
    pub fn receipts(&self, batch: &str) -> Vec<Value> {
        std::fs::read_to_string(self.work().join(format!("receipts/{batch}.jsonl")))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a receipt line"))
            .collect()
    }

    /// Write the batch's answers file.
    pub fn answer(&self, batch: &str, entries: &[Value]) {
        let path = self.work().join(format!("dispositions/{batch}.json"));
        let mut held: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("prepare wrote it"))
                .expect("JSON");
        held["dispositions"] = Value::Array(entries.to_vec());
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&held).expect("JSON") + "\n",
        )
        .expect("written");
    }

    /// Write the batch's ticket `slug`.
    pub fn ticket(&self, batch: &str, ticket: &Ticket) {
        let path = self
            .work()
            .join(format!("tasks/{batch}/tickets/{}.md", ticket.slug));
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the tickets directory");
        std::fs::write(path, ticket.render(batch)).expect("the ticket");
    }

    /// Record a draft and answer its id.
    pub fn draft(&mut self, batch: &str, title: &str, paths: &[&str], body: &str) -> String {
        self.draft_in(batch, REPOSITORY, title, paths, body)
    }

    /// Record a draft about `repository` and answer its id.
    pub fn draft_in(
        &mut self,
        batch: &str,
        repository: &str,
        title: &str,
        paths: &[&str],
        body: &str,
    ) -> String {
        let mut args = vec![
            "draft",
            "--batch",
            batch,
            "--title",
            title,
            "--repository",
            repository,
            "--author",
            "node:worker-1",
            "--stamp",
            "session=s-1",
        ];
        for path in paths {
            args.push("--path");
            args.push(path);
        }
        let printed = self.ok("draft", &args, body);
        printed["draft"].as_str().expect("a draft id").to_owned()
    }
}

/// A ticket the scripted agent writes.
#[derive(Debug, Clone)]
pub struct Ticket {
    /// Its root cause.
    pub slug: String,
    /// Its title, after `widgets: `.
    pub title: String,
    /// The drafts and inputs it stands on.
    pub drafts: Vec<String>,
    /// Its root cause prose.
    pub cause: String,
    /// Its severity and severity with the workaround, and the workaround.
    pub severity: (String, String, String),
    /// Its frequency, when judged.
    pub frequency: Option<String>,
    /// Its kind, when recorded.
    pub kind: Option<String>,
    /// Its declared fields.
    pub fields: Vec<(String, String)>,
    /// Its status role, when it asks for one.
    pub status: Option<String>,
    /// Extra record keys, written as given.
    pub extra: Vec<(String, Value)>,
    /// Its evidence.
    pub evidence: String,
    /// Sections it leaves out.
    pub without: Vec<String>,
    /// The repository it is filed in.
    pub repository: String,
}

impl Ticket {
    /// A sound default-kind ticket.
    pub fn new(slug: &str, title: &str, drafts: &[&str]) -> Self {
        Self {
            slug: slug.to_owned(),
            title: title.to_owned(),
            drafts: drafts.iter().map(|one| (*one).to_owned()).collect(),
            cause: format!("Widgets {title}, so a person loses work they saved."),
            severity: (
                "high".to_owned(),
                "medium".to_owned(),
                "avoid resizing while the save indicator shows".to_owned(),
            ),
            frequency: Some("intermittent".to_owned()),
            kind: None,
            fields: Vec::new(),
            status: None,
            extra: Vec::new(),
            evidence: format!("Host {HOST}: the run's log shows {title}."),
            without: Vec::new(),
            repository: REPOSITORY.to_owned(),
        }
    }

    /// The ticket's severity and severity with the workaround, with `workaround`.
    #[must_use]
    pub fn severities(mut self, severity: &str, mitigated: &str, workaround: &str) -> Self {
        self.severity = (
            severity.to_owned(),
            mitigated.to_owned(),
            workaround.to_owned(),
        );
        self
    }

    /// The ticket's file text.
    pub fn render(&self, batch: &str) -> String {
        let mut record = json!({
            "schema": 14,
            "root_cause": self.slug,
            "repository": self.repository,
            "created_by_run": batch,
            "owning_runs": [batch],
            "drafts": self.drafts,
            "basis": {self.repository.clone(): COMMIT},
            "verified_at": "2026-11-01T11:30:00Z",
            "host": HOST,
        });
        if let Some(frequency) = &self.frequency {
            record["frequency"] = json!(frequency);
        }
        if let Some(kind) = &self.kind {
            record["kind"] = json!(kind);
        }
        if !self.fields.is_empty() {
            record["fields"] = Value::Object(
                self.fields
                    .iter()
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect(),
            );
        }
        for (key, value) in &self.extra {
            record[key] = value.clone();
        }
        let status = self
            .status
            .as_ref()
            .map(|status| format!("status: {status}\n"))
            .unwrap_or_default();
        let (severity, mitigated, workaround) = &self.severity;
        let mut sections = vec![
            ("Root cause".to_owned(), self.cause.clone()),
            (
                "Impact".to_owned(),
                format!(
                    "**Who is affected.** Users of widgets who save layouts.\n**What it costs them.** One lost layout per occurrence.\n**What it costs the product owner.** Some users lose a saved layout each week.\n**Cost of the workaround.**\n- Applying it: a moment per save, by the user\n- Side effects on the outcome: none\n- Discovery: the failure names it\n\n- Severity: {severity}\n- Workaround: {workaround}\n- Severity with the workaround: {mitigated}"
                ),
            ),
            ("Examples".to_owned(), format!("{}.", self.title)),
            (
                "Suggested fix".to_owned(),
                "Read the window size once per save.\n\n**Contributing locations.**\n- widgets `src/layout.rs`: reads the size twice".to_owned(),
            ),
            ("Owning batches".to_owned(), format!("- {batch}")),
        ];
        if !self.fields.is_empty() {
            sections.push((
                "Fields".to_owned(),
                self.fields
                    .iter()
                    .map(|(name, value)| format!("- {name}: {value}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }
        sections.push(("Evidence".to_owned(), self.evidence.clone()));
        let body: String = sections
            .into_iter()
            .filter(|(name, _)| !self.without.contains(name))
            .map(|(name, text)| format!("## {name}\n\n{text}\n\n"))
            .collect();
        format!(
            "---\ntitle: \"{}: {}\"\n{status}metadata:\n  orchestrator.follow-up: {}\n---\n{}",
            self.repository.rsplit('/').next().unwrap_or_default(),
            self.title,
            record,
            body.trim_end()
        ) + "\n"
    }
}

/// A sound answer.
pub fn answer(
    input: &str,
    kind: &str,
    disposition: &str,
    root_causes: &[&str],
    detail: &str,
) -> Value {
    json!({
        "draft": input,
        "disposition": disposition,
        "root_causes": root_causes,
        "detail": detail,
        "kind": kind,
    })
}

/// The front matter of an item another batch filed, with its record.
pub fn filed_item(
    title: &str,
    batch: &str,
    root_cause: &str,
    extra: &Value,
    priority: &str,
) -> String {
    let mut record = json!({
        "schema": 14,
        "root_cause": root_cause,
        "repository": REPOSITORY,
        "created_by_run": batch,
        "owning_runs": [batch],
        "drafts": [format!("drafts:{batch}/drafts/x")],
        "basis": {REPOSITORY: COMMIT},
        "verified_at": "2026-10-01T11:30:00Z",
        "host": HOST,
        "priority_estimate": "medium",
        "board_estimate": "medium",
        "frequency": "intermittent",
    });
    if let Value::Object(more) = extra {
        for (key, value) in more {
            record[key] = value.clone();
        }
    }
    format!(
        "title: \"{title}\"\nstatus: backlog\npriority: {priority}\nmetadata:\n  orchestrator.follow-up: {record}"
    )
}

/// The body of an item another batch filed, estimating `medium` from one occurrence.
pub fn filed_body(cause: &str) -> String {
    format!(
        "## Root cause\n\n{cause}\n\n## Impact\n\n**Who is affected.** Users.\n**What it costs them.** Time.\n**What it costs the product owner.** Support.\n**Cost of the workaround.**\n- Applying it: none\n- Side effects on the outcome: none\n- Discovery: free\n\n- Severity: high\n- Workaround: retry the export\n- Severity with the workaround: medium\n- Priority estimate: medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)\n\n## Evidence\n\nSeen once.\n"
    )
}
