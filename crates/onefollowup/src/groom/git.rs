//! The repositories a grooming pass verifies against, through the `git` on `PATH`: one
//! blob-less clone of each, fetched to its default branch — the one its origin's `HEAD`
//! names, read afresh on every fetch so a repository whose default branch changes is
//! followed — whose history and trees answer every question the pass asks of it without
//! fetching a file's contents.
//!
//! A repository `host/owner/name` is fetched from `https://host/owner/name.git`; git's own
//! `url.<base>.insteadOf` configuration is how a host points that elsewhere.

use crate::error::Error;
use crate::verify::command::{self, Ran};
use std::path::{Path, PathBuf};

/// The URL a repository is fetched from.
#[must_use]
pub fn url(repository: &str) -> String {
    format!("https://{repository}.git")
}

/// The `git` the verb runs: the one on its environment's `PATH`, in its working directory and
/// environment, which are not this process's own when the library was handed others.
#[derive(Debug, Clone, Copy)]
pub struct Git<'a> {
    /// The verb's working directory.
    pub working_directory: &'a Path,
    /// The verb's environment.
    pub environment: &'a [(String, String)],
}

impl Git<'_> {
    fn run(&self, directory: Option<&Path>, args: &[&str]) -> Result<Ran, Error> {
        let mut argv = vec!["git".to_owned()];
        if let Some(directory) = directory {
            argv.push("-C".to_owned());
            argv.push(directory.display().to_string());
        }
        argv.extend(args.iter().map(|&arg| arg.to_owned()));
        let mut environment = self.environment.to_vec();
        environment.push(("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()));
        command::run(&argv, "", self.working_directory, &environment).map_err(|error| {
            Error::Failed(format!("{error}; next: install git and put it on PATH"))
        })
    }
}

fn ran(output: &Ran, what: &str) -> Result<String, Error> {
    if output.exit == 0 {
        Ok(output.stdout.trim().to_owned())
    } else {
        Err(Error::Failed(format!(
            "{what} failed: {}; next: check the repository is reachable and its default branch has a commit",
            output.stderr.trim()
        )))
    }
}

/// Where `repository`'s clone is kept under `clones`.
#[must_use]
pub fn clone_path(clones: &Path, repository: &str) -> PathBuf {
    repository
        .split('/')
        .fold(clones.to_path_buf(), |path, part| path.join(part))
}

/// The branch `HEAD` names in `ls-remote --symref` output, `refs/heads/` stripped.
fn symbolic_head(listed: &str) -> Option<&str> {
    listed.lines().find_map(|line| {
        line.strip_prefix("ref: refs/heads/")?
            .strip_suffix("\tHEAD")
            .filter(|branch| !branch.is_empty())
    })
}

fn no_default_branch(repository: &str) -> Error {
    Error::Failed(format!(
        "{repository}: its origin's `HEAD` names no branch; next: check the repository has a default branch with a commit"
    ))
}

/// Clone `repository` blob-less into `clone`, or fetch the clone already there, check out its
/// default branch, and answer the commit fetched.
///
/// # Errors
///
/// `git` that cannot run, a clone or fetch that fails, or an origin whose `HEAD` names no
/// branch.
pub fn fetch(git: Git<'_>, repository: &str, clone: &Path) -> Result<String, Error> {
    let source = url(repository);
    let remote = if clone.join(".git").exists() {
        // Read the origin's default branch afresh: a repository whose default branch changed
        // since the clone was made is verified at its new one.
        let listed = ran(
            &git.run(Some(clone), &["ls-remote", "--symref", "origin", "HEAD"])?,
            &format!("reading {source}'s default branch"),
        )?;
        let branch = symbolic_head(&listed).ok_or_else(|| no_default_branch(repository))?;
        let remote = format!("refs/remotes/origin/{branch}");
        let fetched = git.run(
            Some(clone),
            &[
                "fetch",
                "--quiet",
                "--filter=blob:none",
                "origin",
                &format!("+refs/heads/{branch}:{remote}"),
            ],
        )?;
        ran(&fetched, &format!("fetching {source}"))?;
        remote
    } else {
        if let Some(parent) = clone.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::Failed(format!(
                    "{}: could not be created: {error}",
                    parent.display()
                ))
            })?;
        }
        // No `--branch`: the clone follows the origin's `HEAD`, and records it as
        // `origin/HEAD`.
        let cloned = git.run(
            None,
            &[
                "clone",
                "--quiet",
                "--filter=blob:none",
                "--no-checkout",
                &source,
                &clone.to_string_lossy(),
            ],
        )?;
        ran(&cloned, &format!("cloning {source}"))?;
        let head = git.run(
            Some(clone),
            &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        )?;
        if head.exit != 0 {
            return Err(no_default_branch(repository));
        }
        head.stdout.trim().to_owned()
    };
    let branch = remote.trim_start_matches("refs/remotes/origin/");
    let commit = ran(
        &git.run(
            Some(clone),
            &["rev-parse", "--verify", &format!("{remote}^{{commit}}")],
        )?,
        &format!("reading {repository}'s default branch `{branch}`"),
    )?;
    ran(
        &git.run(
            Some(clone),
            &["checkout", "--quiet", "--force", "--detach", &commit],
        )?,
        &format!("checking out {repository}'s default branch `{branch}`"),
    )?;
    Ok(commit)
}

/// Clone `repository` bare and blob-less into the new directory `clone`, for a dry run that
/// keeps no clone, and answer its default branch's commit. Its trees answer every question
/// selection asks, and nothing is checked out.
///
/// # Errors
///
/// `git` that cannot run, a clone that fails, or an origin whose `HEAD` names no branch.
pub fn fetch_temporary(git: Git<'_>, repository: &str, clone: &Path) -> Result<String, Error> {
    let source = url(repository);
    // No `--branch`: a bare clone's `HEAD` names the origin's default branch.
    let cloned = git.run(
        None,
        &[
            "clone",
            "--quiet",
            "--bare",
            "--filter=blob:none",
            &source,
            &clone.to_string_lossy(),
        ],
    )?;
    ran(&cloned, &format!("cloning {source}"))?;
    let head = git.run(Some(clone), &["symbolic-ref", "--quiet", "HEAD"])?;
    let local = head.stdout.trim();
    if head.exit != 0 || !local.starts_with("refs/heads/") {
        return Err(no_default_branch(repository));
    }
    let branch = local.trim_start_matches("refs/heads/");
    ran(
        &git.run(
            Some(clone),
            &["rev-parse", "--verify", &format!("{local}^{{commit}}")],
        )?,
        &format!("reading {repository}'s default branch `{branch}`"),
    )
}

/// A directory of the verb's temporary storage, removed with everything in it when dropped.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A new directory under the environment's `TMPDIR`, or the system's temporary directory.
    ///
    /// # Errors
    ///
    /// A directory that cannot be created.
    pub fn new(environment: &[(String, String)]) -> Result<Self, Error> {
        let base = environment
            .iter()
            .find(|(name, value)| name == "TMPDIR" && !value.is_empty())
            .map_or_else(std::env::temp_dir, |(_, value)| PathBuf::from(value));
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path = base.join(format!(
            "onefollowup-groom-dry-run-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).map_err(|error| {
            Error::Failed(format!("{}: could not be created: {error}", path.display()))
        })?;
        Ok(Self { path })
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Temporary storage: a removal the system refuses leaves it to the system's own sweep,
        // and the verb's answer does not depend on it.
        let _ignored = std::fs::remove_dir_all(&self.path);
    }
}

/// Whether `commit` is an ancestor of `head` (or is it) in `clone`. A commit the clone does
/// not hold is not.
///
/// # Errors
///
/// `git` that cannot run.
pub fn is_ancestor(git: Git<'_>, clone: &Path, commit: &str, head: &str) -> Result<bool, Error> {
    let output = git.run(Some(clone), &["merge-base", "--is-ancestor", commit, head])?;
    Ok(output.exit == 0)
}

/// The paths `head` changed since `since`, from trees alone; `None` when the clone does not
/// hold `since`, as after a rewritten history.
///
/// # Errors
///
/// `git` that cannot run.
pub fn changed_paths(
    git: Git<'_>,
    clone: &Path,
    since: &str,
    head: &str,
) -> Result<Option<Vec<String>>, Error> {
    let output = git.run(
        Some(clone),
        &["diff", "--name-only", "--no-renames", since, head],
    )?;
    if output.exit != 0 {
        return Ok(None);
    }
    Ok(Some(
        output
            .stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
    ))
}

/// Whether one of `changed` is `path` or lies under it.
#[must_use]
pub fn touches(changed: &[String], path: &str) -> bool {
    let path = path.trim_end_matches('/');
    changed.iter().any(|changed| {
        changed == path
            || changed
                .strip_prefix(path)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
