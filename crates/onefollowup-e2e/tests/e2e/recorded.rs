//! Real requests `onefollowup prepare` sent the public `followups` board, as GitHub answered and
//! charged them: `fixtures/real-board/requests.json`, recorded by the `spike-real-board-cost`
//! harness's forwarder under onetaskgraph 0.17.3's documents and sanitized (its `about` says
//! how). The loopback prices a request by the recorded one of the same document and page-size
//! binding ([`binding`]): the points GitHub reported and the seconds it took.
//!
//! The fixture holds one priced request per binding the recorded run sent — each at the median
//! seconds of its binding — and the first-page round one follow-up round continues, so a
//! recorded cursor can be followed from one page to the next.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde_json::Value;

/// The fixture, as committed.
pub static FIXTURE: LazyLock<Value> = LazyLock::new(|| {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/e2e/fixtures/real-board/requests.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("the recorded requests"))
        .expect("JSON")
});

/// What GitHub charged one request of a binding, and how long it took to answer it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub points: u64,
    pub seconds: f64,
}

/// One recorded request: its document's name, the document as sent, its variables, what GitHub
/// reported it cost, the rows it answered, its response's bytes and seconds, and the sanitized
/// answer.
pub struct Recording<'a> {
    pub seq: u64,
    pub document: &'a str,
    pub sent: &'a str,
    pub variables: &'a Value,
    pub cost: u64,
    pub rows: u64,
    pub response_bytes: u64,
    pub seconds: f64,
    pub answer: &'a Value,
}

/// Every recorded request, in the order it was sent.
pub fn recordings() -> Vec<Recording<'static>> {
    FIXTURE["requests"]
        .as_array()
        .expect("the recorded requests")
        .iter()
        .map(|request| Recording {
            seq: request["seq"].as_u64().expect("a sequence number"),
            document: request["document"].as_str().expect("a document name"),
            sent: FIXTURE["documents"][request["sent"].as_str().expect("its document")]
                .as_str()
                .expect("the document sent"),
            variables: &request["variables"],
            cost: request["cost"].as_u64().expect("GitHub's reported cost"),
            rows: request["rows"].as_u64().expect("its rows"),
            response_bytes: request["response_bytes"].as_u64().expect("its bytes"),
            seconds: request["seconds"].as_f64().expect("its seconds"),
            answer: &request["answer"],
        })
        .collect()
}

/// The repositories the fixture's issues are in, each as GitHub reported its visibility.
pub fn repositories() -> BTreeMap<String, String> {
    FIXTURE["repositories"]
        .as_object()
        .expect("the repositories")
        .iter()
        .map(|(name, visibility)| (name.clone(), visibility.as_str().unwrap_or("").to_owned()))
        .collect()
}

/// A request's binding: its document's name and the page sizes it was sent with — for a
/// batched search, the distinct page sizes of the slots it asks; for any other document, every
/// integer and boolean variable, which are its page sizes and the connections it includes.
/// The fixture's builder spells it the same way.
pub fn binding(document: &str, variables: &Value) -> String {
    if document == "SEARCH_BATCH" {
        let asked = |flag: &str, size: &str, slots: usize| -> Vec<u64> {
            let sizes: std::collections::BTreeSet<u64> = (0..slots)
                .filter(|slot| variables[format!("{flag}{slot}")] == true)
                .filter_map(|slot| variables[format!("{size}{slot}")].as_u64())
                .collect();
            sizes.into_iter().collect()
        };
        let list = |sizes: Vec<u64>| {
            sizes
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        return format!(
            "SEARCH_BATCH first=[{}] itemsFirst=[{}]",
            list(asked(
                "ask",
                "first",
                onetaskgraph_github_projects::SEARCH_BATCH
            )),
            list(asked(
                "origin",
                "itemsFirst",
                onetaskgraph_github_projects::SEARCH_BATCH_ORIGINS
            )),
        );
    }
    let mut scalars: Vec<(&String, &Value)> = variables
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| value.is_boolean() || value.is_u64() || value.is_i64())
        .collect();
    scalars.sort_by(|a, b| a.0.cmp(b.0));
    let mut spelled = document.to_owned();
    for (name, value) in scalars {
        spelled.push_str(&format!(" {name}={value}"));
    }
    spelled
}

/// What GitHub charged, and the seconds it took, for the recorded request of `binding`.
pub fn price_of(binding: &str) -> Option<Price> {
    let price = FIXTURE["prices"].get(binding)?;
    Some(Price {
        points: price["cost"].as_u64()?,
        seconds: price["seconds"].as_f64()?,
    })
}

/// [`price_of`] the binding of `document` sent with `variables`: refused, naming the binding,
/// when no recorded request has it — the fixture must hold every binding a journey sends.
pub fn price(document: &str, variables: &Value) -> Price {
    let binding = binding(document, variables);
    price_of(&binding).unwrap_or_else(|| {
        panic!(
            "no recorded real request of the binding `{binding}`; record one with the spike-real-board-cost harness's forwarder and add it to fixtures/real-board/requests.json"
        )
    })
}

/// The seconds GitHub took to answer `GET /rate_limit` in the recorded run, at the median.
pub fn rate_limit_seconds() -> f64 {
    FIXTURE["rate_limit_seconds"]
        .as_f64()
        .expect("the rate-limit read's seconds")
}
