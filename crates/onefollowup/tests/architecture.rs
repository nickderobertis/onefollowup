//! The store boundary and the dependency graph.
//!
//! Every store access goes through `src/store.rs`, which calls only onetaskgraph's public
//! crate API; no other source file calls onetaskgraph, names a destination's API host or
//! endpoint, names a plugin's internal crate, or branches on a plugin or destination name.
//! The lockfile holds no onepipeline crate, and no onefollowup crate reaches an HTTP,
//! GraphQL, GitHub or Linear client except through the `onetaskgraph-*` crates.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const STORE_MODULE: &str = "src/store.rs";

fn sources(directory: &Path, into: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("a readable source tree") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

fn crate_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    sources(&root.join("src"), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .expect("under the crate")
                .to_string_lossy()
                .replace('\\', "/");
            (relative, std::fs::read_to_string(&path).expect("readable"))
        })
        .collect()
}

/// What only the store module may name, and what nothing may.
fn findings(relative: &str, text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut found = Vec::new();
    if relative != STORE_MODULE && text.contains("onetaskgraph_") {
        found.push(format!(
            "{relative} calls onetaskgraph; only {STORE_MODULE} may"
        ));
    }
    for plugin in [
        "onetaskgraph_local_md",
        "onetaskgraph_linear",
        "onetaskgraph_github_projects",
        "onetaskgraph_in_memory",
        "onetaskgraph_status_options",
    ] {
        if text.contains(plugin) {
            found.push(format!("{relative} names the plugin crate `{plugin}`; reach every plugin through onetaskgraph's engine"));
        }
    }
    for internal in ["::engine::", "::registry::", "::subprocess::", "::cache::"] {
        if relative == STORE_MODULE && text.contains(&format!("onetaskgraph_core{internal}")) {
            found.push(format!("{relative} reaches into onetaskgraph's `{internal}` module; use the crate root's re-exports"));
        }
    }
    for host in [
        "api.github.com",
        "api.linear.app",
        "graphql",
        "uploads.github",
        "/repos/",
    ] {
        if lowered.contains(host) {
            found.push(format!("{relative} names the destination API `{host}`"));
        }
    }
    for name in [
        "github-projects",
        "local-md",
        "linear",
        "github",
        "in-memory",
    ] {
        if lowered.contains(&format!("\"{name}\"")) {
            found.push(format!(
                "{relative} spells the plugin or destination name \"{name}\" to branch on"
            ));
        }
    }
    found
}

#[test]
fn only_the_store_module_calls_onetaskgraph_and_nothing_names_a_destination() {
    let sources = crate_sources();
    assert!(
        sources
            .iter()
            .any(|(relative, text)| relative == STORE_MODULE && text.contains("onetaskgraph_core"))
    );
    let findings: Vec<String> = sources
        .iter()
        .flat_map(|(relative, text)| findings(relative, text))
        .collect();
    assert!(findings.is_empty(), "{}", findings.join("\n"));
}

#[test]
fn the_rule_catches_what_it_is_for() {
    assert!(!findings("src/api.rs", "use onetaskgraph_core::Engine;").is_empty());
    assert!(!findings(STORE_MODULE, "use onetaskgraph_linear::status_names;").is_empty());
    assert!(!findings(STORE_MODULE, "use onetaskgraph_core::engine::copy;").is_empty());
    assert!(
        !findings(
            "src/api.rs",
            "const HOST: &str = \"https://api.github.com\";"
        )
        .is_empty()
    );
    assert!(!findings("src/api.rs", "if plugin == \"linear\" {}").is_empty());
    assert!(
        findings(
            STORE_MODULE,
            "use onetaskgraph_core::{Engine, SourceState};"
        )
        .is_empty()
    );
}

/// Each lockfile package's name and the names of what it depends on.
fn lockfile() -> BTreeMap<String, BTreeSet<String>> {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"))
            .expect("the workspace's lockfile");
    let mut packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut name = String::new();
    let mut in_dependencies = false;
    for line in text.lines() {
        if line == "[[package]]" {
            in_dependencies = false;
        } else if let Some(value) = line.strip_prefix("name = ") {
            name = value.trim_matches('"').to_owned();
            packages.entry(name.clone()).or_default();
        } else if line == "dependencies = [" {
            in_dependencies = true;
        } else if in_dependencies && line == "]" {
            in_dependencies = false;
        } else if in_dependencies {
            let dependency = line.trim().trim_end_matches(',').trim_matches('"');
            let dependency = dependency
                .split(' ')
                .next()
                .unwrap_or(dependency)
                .to_owned();
            packages.entry(name.clone()).or_default().insert(dependency);
        }
    }
    packages
}

/// A crate that is an HTTP, GraphQL, GitHub or Linear client.
fn is_client(name: &str) -> bool {
    [
        "reqwest", "hyper", "ureq", "isahc", "surf", "curl", "graphql", "octocrab", "github",
        "linear", "cynic", "gql",
    ]
    .iter()
    .any(|word| name.contains(word))
        && !name.starts_with("onetaskgraph")
}

#[test]
fn the_dependency_graph_reaches_clients_only_through_onetaskgraph_and_holds_no_onepipeline() {
    let packages = lockfile();
    assert!(
        packages.keys().all(|name| !name.contains("onepipeline")),
        "no onepipeline crate is in the graph"
    );
    assert!(
        packages.keys().any(|name| is_client(name)),
        "the graph does hold clients, through onetaskgraph"
    );
    for ours in packages
        .keys()
        .filter(|name| name.starts_with("onefollowup"))
    {
        let mut reached = BTreeSet::new();
        let mut stack: Vec<String> = packages[ours]
            .iter()
            .filter(|dependency| !dependency.starts_with("onetaskgraph"))
            .cloned()
            .collect();
        while let Some(next) = stack.pop() {
            if next.starts_with("onetaskgraph") || !reached.insert(next.clone()) {
                continue;
            }
            stack.extend(packages.get(&next).into_iter().flatten().cloned());
        }
        let clients: Vec<&String> = reached.iter().filter(|name| is_client(name)).collect();
        assert!(
            clients.is_empty(),
            "{ours} reaches {clients:?} other than through onetaskgraph"
        );
    }
}
