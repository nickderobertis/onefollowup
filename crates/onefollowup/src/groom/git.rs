//! The repositories a grooming pass verifies against, through the `git` on `PATH`: one
//! blob-less clone of each, fetched to its `main`, whose history and trees answer every
//! question the pass asks of it without fetching a file's contents.
//!
//! A repository `host/owner/name` is fetched from `https://host/owner/name.git`; git's own
//! `url.<base>.insteadOf` configuration is how a host points that elsewhere.

use crate::error::Error;
use crate::verify::command::{self, Ran};
use std::path::{Path, PathBuf};

/// The branch every repository is verified at.
pub const MAIN: &str = "main";
const REMOTE_MAIN: &str = "refs/remotes/origin/main";

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
            "{what} failed: {}; next: check the repository is reachable and has a `{MAIN}` branch",
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

/// Clone `repository` blob-less into `clone`, or fetch the clone already there, check out its
/// `main`, and answer the commit fetched.
///
/// # Errors
///
/// `git` that cannot run, or a clone or fetch that fails.
pub fn fetch(git: Git<'_>, repository: &str, clone: &Path) -> Result<String, Error> {
    let source = url(repository);
    if clone.join(".git").exists() {
        let fetched = git.run(
            Some(clone),
            &[
                "fetch",
                "--quiet",
                "--filter=blob:none",
                "origin",
                &format!("+refs/heads/{MAIN}:{REMOTE_MAIN}"),
            ],
        )?;
        ran(&fetched, &format!("fetching {source}"))?;
    } else {
        if let Some(parent) = clone.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::Failed(format!(
                    "{}: could not be created: {error}",
                    parent.display()
                ))
            })?;
        }
        let cloned = git.run(
            None,
            &[
                "clone",
                "--quiet",
                "--filter=blob:none",
                "--no-checkout",
                "--branch",
                MAIN,
                &source,
                &clone.to_string_lossy(),
            ],
        )?;
        ran(&cloned, &format!("cloning {source}"))?;
    }
    let commit = ran(
        &git.run(Some(clone), &["rev-parse", "--verify", REMOTE_MAIN])?,
        &format!("reading {repository}'s `{MAIN}`"),
    )?;
    ran(
        &git.run(
            Some(clone),
            &["checkout", "--quiet", "--force", "--detach", &commit],
        )?,
        &format!("checking out {repository}'s `{MAIN}`"),
    )?;
    Ok(commit)
}

/// Whether `commit` is an ancestor of `main` (or is it) in `clone`. A commit the clone does
/// not hold is not.
///
/// # Errors
///
/// `git` that cannot run.
pub fn is_ancestor(git: Git<'_>, clone: &Path, commit: &str, main: &str) -> Result<bool, Error> {
    let output = git.run(Some(clone), &["merge-base", "--is-ancestor", commit, main])?;
    Ok(output.exit == 0)
}

/// The paths `main` changed since `since`, from trees alone; `None` when the clone does not
/// hold `since`, as after a rewritten history.
///
/// # Errors
///
/// `git` that cannot run.
pub fn changed_paths(
    git: Git<'_>,
    clone: &Path,
    since: &str,
    main: &str,
) -> Result<Option<Vec<String>>, Error> {
    let output = git.run(
        Some(clone),
        &["diff", "--name-only", "--no-renames", since, main],
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
