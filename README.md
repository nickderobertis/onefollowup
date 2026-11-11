# onefollowup

The follow-up loop for any orchestrator repository: record what a run noticed but could
not fix as a **draft**, verify the drafts, file **one ticket per root cause** on the
destination its team watches, and answer people's comments on those tickets. Every store
access goes through [onetaskgraph](https://github.com/nickderobertis/onetaskgraph), so a
destination is any source onetaskgraph serves (a local Markdown folder, GitHub Projects,
Linear) and onefollowup holds no client of its own.

It ships as a CLI and a Rust library with the same surface. **v1 lands the contract**: every
verb, the configuration file and the record formats. `config show` and `kinds` are
implemented; every other verb parses its arguments and refuses with exit 2 and
`not implemented in this release` until the release that implements it.

## Install

```sh
uv tool install onefollowup-cli      # or: pip install onefollowup-cli
npm install -g onefollowup-cli
cargo install onefollowup --locked
```

The PyPI and npm packages carry the prebuilt `onefollowup` executable alone (Linux x86_64
and arm64, macOS arm64, Windows x86_64); there is no Python or JavaScript API. The Rust
library is the `onefollowup` crate.

## Commands

| Command | What it does |
|---|---|
| `draft --batch <ID> --title <T> --repository <ORIGIN> [--path <P>]... [--author <LABEL>] [--stamp <KEY=VALUE>]...` | Record one draft; its body is read from stdin. |
| `prepare --batch <ID> [--alias <ID>]... [--unanswered]` | Snapshot the batch's inputs (its drafts and the `batch_preparing` hook's inputs), make its destination reads in one batched search and one comments read, and write the verifying agent's task. Prints `{"schema_version": 1, "batch", "drafts", "inputs", "answers", "task_template"}`. |
| `ticket validate --batch <ID> <TICKET>` | Hold one ticket to the rules. |
| `ticket board-status --batch <ID> <ITEM>` | Read one destination item's state and comments from the batch's prepared reads. |
| `ticket search --batch <ID> --ticket <TICKET> <QUERY>` | Search the destination for a ticket's duplicates. |
| `ticket copy --batch <ID> <TICKET>` | **Stage** a ticket's copy to its destination. |
| `ticket comment --batch <ID> --item <ITEM> --root-cause <RC>` | **Stage** the batch's evidence comment (read from stdin) on an item. |
| `ticket re-estimate --batch <ID> <ITEM>` | **Stage** an item's re-estimate. |
| `ticket relate --batch <ID> --relation <NAME> --from <TICKET> --to <TICKET>` | **Stage** an edge of a configured relation. |
| `check --batch <ID>` | Hold the batch's dispositions, tickets and staged writes to the rules before anything is written; exit 0 when sound. |
| `publish --batch <ID>` | Send the staged writes in batched store calls and receipt every outcome. |
| `settle --batch <ID>` | Read back only the writes no answer confirmed, run the `batch_settled` hook, retire the batch's working files. |
| `comments gather [--batch <ID> \| --item <ID>] [--dry-run]` | Gather people's comments to answer into a snapshot. |
| `comments launches --snapshot <S>` | Plan the answering dispatches a snapshot launches. |
| `comments check --feedback <F>` | Hold a gathering's responses to the rules. |
| `comments settle --snapshot <S> [--launched <BATCH>=<FEEDBACK>]...` | Settle a snapshot's answers. |
| `statuses` | Read the destination's statuses against the configured status roles. |
| `kinds` | Print the effective kinds, relations, dispositions and routes, as the verifying agent reads them. |
| `config show [--sources]` | Print the effective configuration as JSON; `--sources` names the layer each key came from. |

The `ticket copy|comment|re-estimate|relate` tools write nothing to the destination: they
stage into the batch's outbox, refusing what the rules refuse, and `publish` sends it.

Every verb prints a JSON document on success. Exit codes: **0** success; **1** failed while
running; **2** refused — a usage error, a refused configuration, or a verb not implemented
in this release.

### Results

Each verb's result is one typed document of the library (`onefollowup::api`), stated here
once by example; `tests/contract.rs` holds every example to the library's type, so the two
cannot drift. A field shown `null`-able in the type (`status`, `version`, `url`, `author`,
`item`, `hook`, a dry run's `snapshot`) is `null` when there is none. Every document carries
`schema_version` 1. The nodes that implement a verb may add optional fields; changing a
field declared here is a contract change agreed with the manager first. `kinds` and
`config show` print the documents the configuration section describes.

#### `check`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "sound": false,
  "problems": [
    {
      "subject": "drafts:nightly-2026-11-01/drafts/20261101T102500Z-flaky-resize-test",
      "message": "the input has no disposition; give it exactly one"
    }
  ]
}
```

#### `comments check`

```json
{
  "schema_version": 1,
  "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md",
  "sound": true,
  "problems": []
}
```

#### `comments gather`

```json
{
  "schema_version": 1,
  "snapshot": "feedback/.snapshots/20261101T120000Z",
  "dry_run": false,
  "items": 3,
  "selected": {
    "nightly-2026-11-01": 2
  },
  "left": 1
}
```

#### `comments launches`

```json
{
  "schema_version": 1,
  "snapshot": "feedback/.snapshots/20261101T120000Z",
  "launches": [
    {
      "batch": "nightly-2026-11-01",
      "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md"
    }
  ]
}
```

#### `comments settle`

```json
{
  "schema_version": 1,
  "snapshot": "feedback/.snapshots/20261101T120000Z",
  "settled": [
    {
      "batch": "nightly-2026-11-01",
      "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md",
      "writes": 2,
      "pending": 0
    }
  ]
}
```

#### `draft`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "draft": "drafts:nightly-2026-11-01/drafts/20261101T101500Z-layout-save-loses-resize",
  "path": ".follow-ups/tasks/nightly-2026-11-01/drafts/20261101T101500Z-layout-save-loses-resize.md"
}
```

#### `prepare`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "drafts": 3,
  "inputs": 1,
  "answers": "dispositions/nightly-2026-11-01.json",
  "task_template": "tasks/nightly-2026-11-01/task.md"
}
```

#### `publish`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "receipts": "receipts/nightly-2026-11-01.jsonl",
  "outcomes": [
    {
      "index": 0,
      "op": "copy",
      "item": "followups:item-42",
      "outcome": "created"
    }
  ]
}
```

#### `settle`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "read_back": 1,
  "confirmed": 1,
  "hook": {
    "argv": [
      "notify",
      "--batch-settled"
    ],
    "exit": 0
  },
  "retired": [
    "outbox/nightly-2026-11-01.json"
  ]
}
```

#### `statuses`

```json
{
  "schema_version": 1,
  "destination": "followups",
  "roles": [
    {
      "role": "proposal",
      "category": "backlog",
      "destination_names": [
        "Proposal"
      ]
    }
  ]
}
```

#### `ticket board-status`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "item": "followups:item-42",
  "title": "widgets: a layout save reads one window size",
  "status": {
    "category": "backlog",
    "name": "Proposal"
  },
  "priority": "high",
  "version": "2026-11-01T11:59:58Z",
  "url": "https://github.com/example/widgets/issues/42",
  "comments": [
    {
      "id": "IC_kwDOA1001",
      "author": "alice",
      "url": "https://github.com/example/widgets/issues/42#issuecomment-1001",
      "last_changed": "2026-11-01T11:00:00Z",
      "body": "Seen again after a resize."
    }
  ]
}
```

#### `ticket comment`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 1,
  "write": {
    "op": "comment",
    "item": "followups:item-13",
    "root_cause": "export-dialog-drops-unicode",
    "body": "Additional evidence from follow-up run `nightly-2026-11-01`.\n\nSeen again.\n\n<!-- orchestrator:follow-up-comment run=\"nightly-2026-11-01\" root_cause=\"export-dialog-drops-unicode\" -->\n"
  }
}
```

#### `ticket copy`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 0,
  "write": {
    "op": "copy",
    "ticket": "drafts:nightly-2026-11-01/tickets/layout-save-reads-size-twice",
    "destination": "followups"
  }
}
```

#### `ticket re-estimate`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 2,
  "write": {
    "op": "re-estimate",
    "item": "followups:item-13",
    "priority_estimate": "high"
  }
}
```

#### `ticket relate`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 3,
  "write": {
    "op": "relate",
    "relation": "of-error",
    "from": "drafts:nightly-2026-11-01/tickets/client-acme-e1042",
    "to": "followups:item-7"
  }
}
```

#### `ticket search`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "ticket": "layout-save-reads-size-twice",
  "query": "layout save window size",
  "hits": [
    {
      "item": "followups:item-42",
      "title": "widgets: a layout save reads one window size",
      "status": {
        "category": "backlog",
        "name": "Proposal"
      },
      "url": "https://github.com/example/widgets/issues/42"
    }
  ]
}
```

#### `ticket validate`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "ticket": "layout-save-reads-size-twice",
  "sound": false,
  "problems": [
    "the ticket records the field `client`, which `fields` does not declare"
  ]
}
```
.

### As a library

Each command is one function in `onefollowup::api`, taking the resolved settings and the
command's own arguments and returning the typed result the command prints:

```rust,no_run
use onefollowup::api::{self, ConfigShowArgs};
use onefollowup::config::{self, Inputs};

let settings = config::resolve(&Inputs::from_process()?)?;
let shown = api::config_show(&settings, ConfigShowArgs { sources: true })?;
println!("{}", shown.config.destination);
# Ok::<(), onefollowup::Error>(())
```

Settings can also be given directly: `Settings::new(config)` holds a `Config` to the same
rules a file is held to.

## Configuration

Every setting is resolved from, first to last, the first layer that states it winning:

1. **A flag.** `--set <KEY=VALUE>` sets any key (a dotted path for a nested one:
   `--set estimate.raise_at=5`); the most used keys have flags of their own —
   `--destination`, `--drafts-source`, `--store` — which beat `--set` for their key.
2. **An environment variable**, `ONEFOLLOWUP_<KEY>`, a nested key's segments joined by
   `__`: `ONEFOLLOWUP_ESTIMATE__RAISE_AT=5`.
3. **The file** `onefollowup.yaml`, found by `--config <PATH>`, else `ONEFOLLOWUP_CONFIG`,
   else the working directory.
4. **The default**, shipped in the binary
   ([`crates/onefollowup/defaults/onefollowup.yaml`](crates/onefollowup/defaults/onefollowup.yaml)).

A flag's or variable's value is YAML, so `5` is a number, `[a, b]` a list and
`{same: [x]}` a mapping. Mappings merge key by key across layers; a list or a scalar is
replaced whole. An unknown key, a wrongly typed value or a broken rule is refused, naming
the key and the layer that stated it. `config show --sources` names, for each top-level
key, the layer its value came from.

| Key | Default | Meaning |
|---|---|---|
| `store` | the store's own discovery | The onetaskgraph configuration: an `onetaskgraph.yaml`, or the directory holding one. |
| `drafts_source` | `drafts` | The source drafts are recorded in. |
| `destination` | `followups` | The source every ticket is copied to unless its kind names another; the store's own routes on that source still apply. |
| `namespace` | `orchestrator` | Prefix of the comment markers (`orchestrator:follow-up-comment`) and the record's metadata key (`orchestrator.follow-up`). |
| `host_label` | the machine's hostname | The host a ticket records as having verified it. |
| `batch_id_pattern` | word characters, `.`, `@`, `+`, `-`, not dots alone | A batch id, matched in full. |
| `statuses` | proposal → `backlog`, accepted → `todo`, deferred → `draft`, queued → `queued`, in-progress → `in-progress`, done → `done`, cancelled → `cancelled` | Each status role mapped to a store status category, with `description`, `protected` (no batch withdraws an item at this role) and `selected` (an agent picking up accepted work selects it). |
| `rubric` | four severity levels, critical → low | Severity `levels` (each with the priority it estimates at and its meaning for users, development and resources), `areas`, `guidance`, `impact_lines`, `no_workaround`, `impact_parts`, `workaround_costs`. |
| `estimate` | `raise_at: 3`, `raise_cap: high` | An estimate is raised one level, capped at `raise_cap`, when a root cause fires consistently or has `raise_at` or more occurrences. |
| `task_template` | `builtin:task-template.md.j2` | The verifying agent's task template; a path, or `builtin:` for a shipped file. |
| `persona` | `builtin:persona.yaml` | The verifying agent's persona. |
| `fields` | `{}` | Declared fields (below). |
| `kinds` | one kind, `root-cause` | Ticket kinds (below). |
| `default_kind` | `root-cause` | The kind a record without one is read as. |
| `relations` | `[]` | Relations between kinds (below). |
| `dispositions` | `filed`, `not-reproducible`, `already-fixed`, `too-low-impact` | The disposition catalogue (below). |
| `default_dispositions` | none | The dispositions every kind allows unless it lists its own; none allows the whole catalogue. |
| `routes` | `[]` | Ordered routes (below). |
| `hooks` | none | Commands run at named moments (below). |
| `snapshot` | `launchable_for: 24h` | How long a comment snapshot stays launchable. |

No key says *when* the loop runs; that is the host's schedule.

### Triage as configuration

Triage is declared, not programmed: kinds, the relations between them, dispositions,
ordered routes and command hooks, with today's strategy as the default. There is no rules
engine and no lifecycle policy.

**`fields`** — `{<name>: {description: <text>}}`, every field a ticket may record beyond
the record's own keys (`error_signature`, `client`, …). The verifying agent is shown each
description; any kind may record any declared field; a ticket recording an undeclared
field is refused by `ticket validate` and `check`. Values live in the ticket record (the
store's metadata slot) and in the ticket's visible section; nothing projects them to labels
or board fields. A field declares no format.

**`kinds`** — a list, and `default_kind` names one. The verifying agent picks one kind per
ticket.

```yaml
- name: client-error
  description: One client's error.          # shown to the verifying agent
  ticket_template: templates/client.md.j2   # optional; default the shipped template
  duplicates: {same: [error_code, client]}  # or `agent` (the default)
  priority: {model: estimate}               # or {fixed: high}
  initial_status: proposal                  # the status role a new ticket is copied in
  destination: errors                       # optional; default the top-level destination
  dispositions: [filed, wont-fix]           # optional; default `default_dispositions`
```

`duplicates` says how a ticket of the kind is found to be one already filed. `agent` is the
agent searching the destination and judging whether an open ticket states the same thing;
its evidence then goes to that ticket by its id. `{same: [...]}` is exact matching on
declared fields, for kinds whose values are already exact: two tickets are one when every
listed field is equal after trimming whitespace and case-folding, and a ticket with any
listed field empty never merges. `priority: {model: estimate}` is the severity × frequency
estimate over `rubric` and `estimate`; `{fixed: <priority>}` gives every ticket that
priority.

**`relations`** — named, typed edges between kinds, pointing from the more specific ticket
to the more general:

```yaml
- name: of-error
  from: client-error
  to: error
  cardinality: many-to-one         # or many-to-many
  decided_by: {match: [error_code]} # or `agent`
  store: parent                    # the store's parent relation, or `link` (a typed link)
  rollup: occurrences              # or `max`, or `none` (the default)
```

`decided_by: {match}` is deterministic and allowed only between two kinds that both find
duplicates by `same`, its fields exactly the `to` kind's `same` fields and all among the
`from` kind's: the related ticket is the `to` ticket whose values of those fields equal the
`from` ticket's. `agent` is the verifying agent asserting edges as judgment. `rollup` feeds
the `to` ticket's priority estimate over direct edges: `occurrences` adds each related
ticket's occurrence count, `max` keeps the estimate no lower than the highest among its
related tickets. Closing a ticket closes no related ticket. Refused: `store: parent` with
`many-to-many`; `match` with `many-to-many`; a `rollup` into a kind of fixed priority.

**`dispositions`** — the catalogue every input is given exactly one of, carrying what its
`requires` names:

```yaml
- name: filed
  description: Filed as a new ticket or as evidence on an existing one.
  requires: [ticket]   # any of: ticket, change, reason
```

`requires: [ticket]` is the only statement that an input is filed; `reason` and `change`
file nothing. `default_dispositions` (a nonempty catalogue subset) is what every kind
allows unless it lists its own `dispositions`; with neither, a kind allows the whole
catalogue. The verifying agent assigns every input a kind, including one it does not file,
and an input's kind need not equal the kind of a ticket it names.

**`routes`** — an ordered list matched deterministically against each ticket; the first
match wins:

```yaml
- name: on-call
  match:                       # every key optional
    kind: [client-error]
    severity: [critical]       # the rubric severity the ticket records
    repository: ["github.com/example/*"]
    path: ["src/**"]
    relations: {of-error: {min: 2}}   # edges at either end, staged ones counted
  set:                         # every key optional; applied over the kind's defaults
    status_role: accepted
    priority: urgent
    labels: [incident]
    assignee: alice            # one person, as the destination names people
  command: [rotation, today]   # optional escape hatch
```

`command` runs before the ticket's write with
`{"schema_version": 1, "ticket": <record>, "kind", "severity", "repository", "paths", "route"}`
on stdin; its stdout is empty or a JSON object of `status_role`, `priority`, `labels` and
`assignee` overriding `set`, and a non-zero exit withholds that ticket's write, receipted
as `route-command-failed`. A ticket has one assignee; a person's own assignment always
stands.

**`hooks`** — a map from the moment a hook fires to its argv. Two moments exist; any other
key is refused:

- `batch_preparing`: `prepare` runs it before it snapshots, with
  `{"schema_version": 1, "batch": "<id>"}` on stdin. Its stdout is the additional inputs
  needing a disposition, `{"schema_version": 1, "inputs": [{"id", "source", "summary",
  "detail"}]}`, `source` saying where the input came from (such as `budget-overrun`). A
  non-zero exit or any other stdout fails `prepare`, naming the hook, with nothing
  snapshotted.
- `batch_settled`: `settle` runs it with the settled batch's summary on stdin.

## Record formats

Every record is declared once as a Rust type in `onefollowup::formats` with its parse and
render, and each reads and writes **only its current schema**, refusing any other with an
error naming the format, the schema found and the schema it reads. There is no migration.
The formats are those on boards and disks today, so nothing already written is rewritten;
the batch id stands where they say `run`.

| Format | Schema | Where |
|---|---|---|
| Ticket record | 14 | The ticket's `<namespace>.follow-up` metadata. Optional keys onefollowup adds within 14: `kind`, `fields`, `edges` (`[{"relation", "to"}]`) and `board_assignee`, each omitted when empty. |
| Draft record | 1 | A draft's `<namespace>.follow-up-draft` metadata. |
| Comment markers | — | `<!-- <namespace>:follow-up-comment run="…" root_cause="…" -->` closing an evidence comment or (with `kind="reply" answers="…" verdict="…"`) a reply; the quoted-comment and feedback anchors. |
| Receipt journal | 3 | `receipts/<batch>.jsonl`, one line per write outcome. |
| Dispositions | 1 | `dispositions/<batch>.json`; each entry may carry `kind`, which `check` requires. |
| Carried dispositions | 1 | `dispositions/<batch>.carried.json`. |
| Responses | 3 | `feedback/<batch>/<stamp>.responses.json`. |
| Snapshot | 2 | `feedback/.snapshots/<stamp>/snapshot.json`, with its `launched.json` (1). |
| Watermark | 1 | `feedback/.watermark-<board>.json`. |
| Pending writes | 1 | `feedback/pending.json`. |
| Existing tickets | 1 | `existing-tickets.json`. |
| Outbox | 1 | `outbox/<batch>.json`: the staged writes `publish` sends. |

## Development

`just bootstrap` from a clean clone, then `just check`, the deterministic gate; `just --list`
names every recipe.
