//! The rules a configuration is held to beyond its types: names that must resolve, and the
//! combinations of kinds, relations, dispositions and routes `README.md` refuses.
//!
//! Each refusal is the key (a dotted path) and a message saying what to do.

use std::collections::HashSet;

use serde_json::Value;

use super::types::{
    CONTEXT_FILE, Cardinality, Config, DecidedBy, Duplicates, PriorityModel, RelationStore, Rollup,
};
use crate::store;

/// A refusal: the key and what is wrong.
pub type Refusal = (String, String);

/// Keys a kind once had, each with what replaced it.
const RETIRED_KIND_KEYS: &[(&str, &str)] = &[
    ("identity", "say how a duplicate is found with `duplicates`"),
    ("dedupe", "say how a duplicate is found with `duplicates`"),
    (
        "classification",
        "a kind is judgment the verifying agent makes; describe it in `description`",
    ),
    ("purpose", "describe the kind in `description`"),
];

/// Refuse, on the merged document before it is typed, keys that were retired or misspelled
/// in a way a type error would explain poorly.
pub fn retired(document: &Value) -> Result<(), Refusal> {
    for (index, kind) in list(document, "kinds") {
        for (key, instead) in RETIRED_KIND_KEYS {
            if kind.get(*key).is_some() {
                return Err((
                    format!("kinds[{index}].{key}"),
                    format!("is not a kind key: {instead}"),
                ));
            }
        }
    }
    for (index, disposition) in list(document, "dispositions") {
        if disposition.get("files_ticket").is_some() {
            return Err((
                format!("dispositions[{index}].files_ticket"),
                "is not a disposition key: `requires: [ticket]` is what says an input is filed"
                    .to_owned(),
            ));
        }
    }
    for (index, route) in list(document, "routes") {
        if route
            .get("set")
            .and_then(|set| set.get("assignees"))
            .is_some()
        {
            return Err((
                format!("routes[{index}].set.assignees"),
                "a ticket has one assignee: name one person with `assignee`".to_owned(),
            ));
        }
    }
    Ok(())
}

fn list<'a>(document: &'a Value, key: &str) -> impl Iterator<Item = (usize, &'a Value)> {
    document
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
}

/// Hold a typed configuration to every cross-key rule.
pub fn validate(config: &Config) -> Result<(), Refusal> {
    regex::Regex::new(&config.batch_id_pattern).or_else(|error| {
        refuse(
            "batch_id_pattern".to_owned(),
            format!("is not a regular expression: {error}"),
        )
    })?;
    for (role, mapping) in config.statuses.iter() {
        if !store::is_status_category(&mapping.category) {
            return refuse(
                format!("statuses.{}.category", role.as_str()),
                format!(
                    "`{}` is not a store status category; use one of {}",
                    mapping.category,
                    store::STATUS_CATEGORIES.join(", ")
                ),
            );
        }
    }
    if config.rubric.levels.is_empty() {
        return refuse(
            "rubric.levels".to_owned(),
            "names no severity level".to_owned(),
        );
    }
    unique(
        config.rubric.levels.iter().map(|level| level.name.as_str()),
        "rubric.levels",
    )?;
    if !duration(&config.snapshot.launchable_for) {
        return refuse(
            "snapshot.launchable_for".to_owned(),
            format!(
                "`{}` is not a duration: a whole number then s, m, h or d, such as 24h",
                config.snapshot.launchable_for
            ),
        );
    }
    for (name, field) in &config.fields {
        if field.description.trim().is_empty() {
            return refuse(
                format!("fields.{name}.description"),
                "is empty: the verifying agent is shown what each field records".to_owned(),
            );
        }
    }

    let catalogue: Vec<&str> = config
        .dispositions
        .iter()
        .map(|one| one.name.as_str())
        .collect();
    if catalogue.is_empty() {
        return refuse(
            "dispositions".to_owned(),
            "catalogues no disposition".to_owned(),
        );
    }
    unique(catalogue.iter().copied(), "dispositions")?;
    if let Some(defaults) = &config.default_dispositions {
        subset(defaults, &catalogue, "default_dispositions")?;
    }

    if config.kinds.is_empty() {
        return refuse(
            "kinds".to_owned(),
            "names no kind; declare at least one".to_owned(),
        );
    }
    unique(config.kinds.iter().map(|kind| kind.name.as_str()), "kinds")?;
    let kind_named = |name: &str| config.kinds.iter().find(|kind| kind.name == name);
    if kind_named(&config.default_kind).is_none() {
        return refuse(
            "default_kind".to_owned(),
            format!("`{}` names no configured kind", config.default_kind),
        );
    }
    for (index, kind) in config.kinds.iter().enumerate() {
        if let Duplicates::Same(fields) = &kind.duplicates {
            let key = format!("kinds[{index}].duplicates.same");
            if fields.is_empty() {
                return refuse(
                    key,
                    "is empty: a `same` rule compares at least one field".to_owned(),
                );
            }
            for field in fields {
                if !config.fields.contains_key(field) {
                    return refuse(
                        key,
                        format!("names `{field}`, which `fields` does not declare"),
                    );
                }
            }
        }
        if let Some(own) = &kind.dispositions {
            subset(own, &catalogue, &format!("kinds[{index}].dispositions"))?;
        }
    }

    unique(
        config
            .relations
            .iter()
            .map(|relation| relation.name.as_str()),
        "relations",
    )?;
    for (index, relation) in config.relations.iter().enumerate() {
        let key = |rest: &str| format!("relations[{index}].{rest}");
        let Some(from) = kind_named(&relation.from) else {
            return refuse(
                key("from"),
                format!("`{}` names no configured kind", relation.from),
            );
        };
        let Some(to) = kind_named(&relation.to) else {
            return refuse(
                key("to"),
                format!("`{}` names no configured kind", relation.to),
            );
        };
        if relation.store == RelationStore::Parent
            && relation.cardinality == Cardinality::ManyToMany
        {
            return refuse(
                key("store"),
                "`parent` holds one parent per ticket, so it needs `cardinality: many-to-one`"
                    .to_owned(),
            );
        }
        if let DecidedBy::Match(fields) = &relation.decided_by {
            if relation.cardinality == Cardinality::ManyToMany {
                return refuse(
                    key("decided_by"),
                    "a `match` derives one related ticket, so it needs `cardinality: many-to-one`"
                        .to_owned(),
                );
            }
            let (Some(from_same), Some(to_same)) = (from.duplicates.same(), to.duplicates.same())
            else {
                return refuse(
                    key("decided_by"),
                    "a `match` is allowed only between two kinds that both find duplicates by `same`"
                        .to_owned(),
                );
            };
            let listed: HashSet<&String> = fields.iter().collect();
            if listed != to_same.iter().collect::<HashSet<_>>()
                || !fields.iter().all(|field| from_same.contains(field))
            {
                return refuse(
                    key("decided_by.match"),
                    format!(
                        "its fields are exactly the `to` kind's `same` fields ({}) and all among the `from` kind's ({})",
                        to_same.join(", "),
                        from_same.join(", ")
                    ),
                );
            }
        }
        if relation.rollup != Rollup::None && matches!(to.priority, PriorityModel::Fixed(_)) {
            return refuse(
                key("rollup"),
                format!(
                    "rolls up into `{}`, whose priority is fixed; use `rollup: none` or give that kind `priority: {{model: estimate}}`",
                    to.name
                ),
            );
        }
    }

    unique(
        config.routes.iter().map(|route| route.name.as_str()),
        "routes",
    )?;
    for (index, route) in config.routes.iter().enumerate() {
        for kind in route.matches.kind.iter().flatten() {
            if kind_named(kind).is_none() {
                return refuse(
                    format!("routes[{index}].match.kind"),
                    format!("`{kind}` names no configured kind"),
                );
            }
        }
        for severity in route.matches.severity.iter().flatten() {
            if !config
                .rubric
                .levels
                .iter()
                .any(|level| &level.name == severity)
            {
                return refuse(
                    format!("routes[{index}].match.severity"),
                    format!("`{severity}` names no rubric level"),
                );
            }
        }
        for relation in route.matches.relations.iter().flat_map(|map| map.keys()) {
            if !config.relations.iter().any(|one| &one.name == relation) {
                return refuse(
                    format!("routes[{index}].match.relations"),
                    format!("`{relation}` names no configured relation"),
                );
            }
        }
        if route.command.as_ref().is_some_and(Vec::is_empty) {
            return refuse(
                format!("routes[{index}].command"),
                "is an empty argv".to_owned(),
            );
        }
        if route
            .set
            .assignee
            .as_ref()
            .is_some_and(|one| one.trim().is_empty())
        {
            return refuse(
                format!("routes[{index}].set.assignee"),
                "names nobody".to_owned(),
            );
        }
    }
    for (name, argv) in config.hooks.named() {
        if argv.is_some_and(Vec::is_empty) {
            return refuse(format!("hooks.{name}"), "is an empty argv".to_owned());
        }
        if argv.is_some_and(|argv| argv.iter().any(|one| one.contains(CONTEXT_FILE)))
            && config.hooks.owner.is_none()
        {
            return refuse(
                format!("hooks.{name}"),
                format!(
                    "names `{CONTEXT_FILE}`, whose file names an owner; set `hooks.owner.session`"
                ),
            );
        }
    }
    if let Some(owner) = &config.hooks.owner {
        for (part, value) in [
            ("session", Some(&owner.session)),
            ("run", owner.run.as_ref()),
            ("plan", owner.plan.as_ref()),
        ] {
            let Some(value) = value else { continue };
            if value.trim().is_empty() {
                return refuse(format!("hooks.owner.{part}"), "is blank".to_owned());
            }
            if value.contains("${") && variable(value).is_none() {
                return refuse(
                    format!("hooks.owner.{part}"),
                    format!(
                        "`{value}` is neither a literal nor one whole `${{NAME}}` reference to an environment variable"
                    ),
                );
            }
        }
    }
    Ok(())
}

/// The environment variable a `hooks.owner` value names as one whole `${NAME}`, if it does.
#[must_use]
pub(crate) fn variable(value: &str) -> Option<&str> {
    let name = value.strip_prefix("${")?.strip_suffix('}')?;
    let mut chars = name.chars();
    (chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|one| one == '_' || one.is_ascii_alphanumeric()))
    .then_some(name)
}

fn refuse<T>(key: String, message: String) -> Result<T, Refusal> {
    Err((key, message))
}

fn unique<'a>(names: impl Iterator<Item = &'a str>, key: &str) -> Result<(), Refusal> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err((key.to_owned(), format!("names `{name}` twice")));
        }
    }
    Ok(())
}

fn subset(names: &[String], catalogue: &[&str], key: &str) -> Result<(), Refusal> {
    if names.is_empty() {
        return Err((
            key.to_owned(),
            "is empty: list at least one catalogued disposition".to_owned(),
        ));
    }
    for name in names {
        if !catalogue.contains(&name.as_str()) {
            return Err((
                key.to_owned(),
                format!("names `{name}`, which `dispositions` does not catalogue"),
            ));
        }
    }
    Ok(())
}

fn duration(text: &str) -> bool {
    text.len() > 1
        && text[..text.len() - 1]
            .chars()
            .all(|one| one.is_ascii_digit())
        && matches!(text.as_bytes()[text.len() - 1], b's' | b'm' | b'h' | b'd')
}
