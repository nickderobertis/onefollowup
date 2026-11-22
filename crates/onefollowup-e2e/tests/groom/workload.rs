//! The generated grooming workload: a local-md destination of tickets over one-commit-history
//! git repositories. `groom prepare`'s journey runs over it, and `groom publish`'s full-pass and
//! repeat-pass budgets use it as their workload, so it is generated here once.

use super::world::World;

/// The status every eighth ticket cycles through: open at five categories, and one closed.
pub const STATUSES: [&str; 8] = [
    "backlog",
    "todo",
    "backlog",
    "draft",
    "queued",
    "backlog",
    "in-progress",
    "done",
];

/// One generated repository.
#[derive(Debug, Clone)]
pub struct Repository {
    /// `host/owner/name`.
    pub repository: String,
    /// Its one commit, `main`.
    pub commit: String,
}

/// One generated ticket.
#[derive(Debug, Clone)]
pub struct Ticket {
    /// Its native id in the destination.
    pub id: String,
    /// Its status.
    pub status: &'static str,
    /// The repository it names.
    pub repository: String,
    /// The path its body names.
    pub path: String,
}

impl Ticket {
    /// Whether the destination holds it open.
    pub fn open(&self) -> bool {
        !matches!(self.status, "done" | "cancelled")
    }
}

/// What was generated.
#[derive(Debug, Clone)]
pub struct Workload {
    /// Every repository.
    pub repositories: Vec<Repository>,
    /// Every ticket, open and closed.
    pub tickets: Vec<Ticket>,
}

impl Workload {
    /// The open tickets.
    pub fn open(&self) -> Vec<&Ticket> {
        self.tickets.iter().filter(|ticket| ticket.open()).collect()
    }
}

/// Generate `repositories` one-commit repositories, each holding `src/module_<k>.rs` for every
/// `k` below `files`, and `tickets` tickets over them, round-robin, each naming one module of
/// its repository in its body, at the statuses [`STATUSES`] cycles through.
pub fn generate(world: &World, repositories: usize, files: usize, tickets: usize) -> Workload {
    let repositories: Vec<Repository> = (0..repositories)
        .map(|index| {
            let repository = format!("github.com/example/service-{index:02}");
            let sources: Vec<(String, String)> = (0..files)
                .map(|k| {
                    (
                        format!("src/module_{k}.rs"),
                        format!("pub fn module_{k}() -> usize {{ {k} }}\n"),
                    )
                })
                .collect();
            let borrowed: Vec<(&str, &str)> = sources
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_str()))
                .collect();
            let commit = world.repository(&repository, &borrowed);
            Repository { repository, commit }
        })
        .collect();
    let tickets: Vec<Ticket> = (0..tickets)
        .map(|index| {
            let repository = repositories[index % repositories.len()].repository.clone();
            let path = format!("src/module_{}.rs", index % files.max(1));
            let ticket = Ticket {
                id: format!("ticket-{index:03}"),
                status: STATUSES[index % STATUSES.len()],
                repository,
                path,
            };
            world.ticket(
                &ticket.id,
                &format!(
                    "service: `{}` answers the wrong value ({index})",
                    ticket.path
                ),
                ticket.status,
                &[&ticket.repository],
                &format!(
                    "`{}` in `{}` answers a value its callers do not expect, number {index}.",
                    ticket.path, ticket.repository
                ),
            );
            ticket
        })
        .collect();
    Workload {
        repositories,
        tickets,
    }
}

/// The open statuses [`destination`]'s open tickets cycle through.
pub const OPEN_STATUSES: [&str; 7] = [
    "backlog",
    "todo",
    "backlog",
    "draft",
    "queued",
    "backlog",
    "in-progress",
];

/// Generate `repositories` one-commit repositories as [`generate`] does, then `open` open
/// tickets cycling through [`OPEN_STATUSES`] and `closed` closed ones, round-robin over the
/// repositories, each holding `comments` comments a person made two days ago.
pub fn destination(
    world: &World,
    repositories: usize,
    files: usize,
    open: usize,
    closed: usize,
    comments: usize,
) -> Workload {
    let generated = generate(world, repositories, files, 0);
    let made = super::world::ago(2 * 86_400);
    let tickets: Vec<Ticket> = (0..open + closed)
        .map(|index| {
            let repository = generated.repositories[index % repositories]
                .repository
                .clone();
            let path = format!("src/module_{}.rs", index % files.max(1));
            let status = if index < open {
                OPEN_STATUSES[index % OPEN_STATUSES.len()]
            } else if index % 2 == 0 {
                "done"
            } else {
                "cancelled"
            };
            let ticket = Ticket {
                id: format!("ticket-{index:04}"),
                status,
                repository,
                path,
            };
            let mut body = format!(
                "`{}` in `{}` answers a value its callers do not expect, number {index}.\n\n## Comments\n",
                ticket.path, ticket.repository
            );
            for at in 0..comments {
                body.push_str(&format!(
                    "\n<!-- onetaskgraph:comment id=\"c-{index:04}-{at}\" author=\"person\" created_at=\"{made}\" updated_at=\"{made}\" -->\n### person — {made}\n\nSeen again, note {at}.\n\n<!-- /onetaskgraph:comment -->\n"
                ));
            }
            world.ticket(
                &ticket.id,
                &format!(
                    "service: `{}` answers the wrong value ({index})",
                    ticket.path
                ),
                ticket.status,
                &[&ticket.repository],
                &body,
            );
            ticket
        })
        .collect();
    Workload {
        repositories: generated.repositories,
        tickets,
    }
}
