//! The store seam, over onetaskgraph's `local-md` source alone: destination behaviour is
//! onetaskgraph's to prove, so nothing here reaches a network destination.

use std::path::Path;

use onefollowup::store::{self, STATUS_CATEGORIES, Store, StoreError};

fn local_md_store(directory: &Path) {
    std::fs::create_dir_all(directory.join("drafts-root")).expect("the source's root");
    std::fs::write(
        directory.join(store::STORE_DOCUMENT),
        "sources:\n  drafts:\n    plugin: local-md\n    config:\n      root: drafts-root\n",
    )
    .expect("the store's configuration");
}

/// Only the variables onetaskgraph reads for its own layers, pointed at nothing, so the
/// developer's user-level onetaskgraph configuration never reaches the test.
fn isolated(home: &Path) -> Vec<(String, String)> {
    vec![
        ("HOME".to_owned(), home.display().to_string()),
        (
            "XDG_CONFIG_HOME".to_owned(),
            home.join("config").display().to_string(),
        ),
        (
            "XDG_CACHE_HOME".to_owned(),
            home.join("cache").display().to_string(),
        ),
    ]
}

#[test]
fn a_local_md_store_opens_from_its_document_its_directory_or_discovery() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    local_md_store(directory.path());
    let environment = isolated(directory.path());
    let elsewhere = tempfile::tempdir().expect("another directory");
    let document = directory.path().join(store::STORE_DOCUMENT);
    for (named, working) in [
        (Some(document.as_path()), elsewhere.path()),
        (Some(directory.path()), elsewhere.path()),
        (None, directory.path()),
    ] {
        let opened = Store::open(named, working, &environment).expect("the store opens");
        let sources = opened.sources();
        assert_eq!(sources.len(), 1, "{named:?}");
        assert_eq!(sources[0].name, "drafts");
        assert!(sources[0].available);
    }
}

#[test]
fn a_store_that_is_not_an_onetaskgraph_configuration_is_refused() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let environment = isolated(directory.path());
    let other = directory.path().join("other.yaml");
    std::fs::write(&other, "sources: {}\n").expect("written");
    for named in [other.as_path(), directory.path()] {
        let error = Store::open(Some(named), directory.path(), &environment)
            .err()
            .expect("refused");
        assert!(matches!(error, StoreError::NotAConfiguration(_)), "{error}");
        assert!(error.to_string().contains("onetaskgraph.yaml"));
    }
    std::fs::write(
        directory.path().join(store::STORE_DOCUMENT),
        "sources: [not, a, map]\n",
    )
    .expect("written");
    let error = Store::open(Some(directory.path()), directory.path(), &environment)
        .err()
        .expect("refused");
    assert!(matches!(error, StoreError::Refused(_)), "{error}");
}

#[test]
fn the_status_categories_are_onetaskgraphs() {
    let schema = serde_json::to_value(schemars::schema_for!(
        onetaskgraph_plugin_api::StatusCategory
    ))
    .expect("the schema is JSON");
    let mut theirs: Vec<String> = schema["enum"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
        .expect("an enumerated vocabulary")
        .iter()
        .flat_map(|one| {
            one.as_str()
                .map(str::to_owned)
                .or_else(|| one["const"].as_str().map(str::to_owned))
        })
        .filter(|name| name != "unknown")
        .collect();
    let mut ours: Vec<String> = STATUS_CATEGORIES
        .iter()
        .map(|one| (*one).to_owned())
        .collect();
    theirs.sort();
    ours.sort();
    assert_eq!(
        ours, theirs,
        "STATUS_CATEGORIES is onetaskgraph's vocabulary less `unknown`"
    );
    for category in STATUS_CATEGORIES {
        assert!(store::is_status_category(category), "{category}");
    }
    for word in ["unknown", "finished", "Done", ""] {
        assert!(!store::is_status_category(word), "{word}");
    }
}
