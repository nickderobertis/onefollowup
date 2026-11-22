//! The grooming journeys, written once and run by two suites: `onefollowup-e2e`'s, which drives
//! the built binary as a subprocess, and the `onefollowup` crate's, which runs the library in
//! process under its coverage. Each suite supplies a [`world::Runner`] and expands
//! [`groom_journeys!`] with it; everything else — the world, its git repositories, its local-md
//! destination and every assertion — is shared.

pub mod check;
pub mod prepare;
pub mod selection;
pub mod workload;
pub mod world;

/// One `#[test]` per grooming journey, each run with `$runner`.
macro_rules! groom_journeys {
    ($runner:expr) => {
        $crate::groom::groom_journeys!(@each $runner;
            prepare::shards_carry_each_agents_task,
            prepare::nothing_to_select_answers_no_shard,
            prepare::a_zero_shard_size_is_refused,
            prepare::an_unreachable_repository_fails_naming_it,
            prepare::a_configured_task_template_and_persona_shape_the_task,
            prepare::a_dry_run_writes_nothing_with_or_without_a_clone,
            check::verdicts_are_held_to_the_rules_and_planned,
            check::malformed_verdict_records_are_refused_by_rule,
            check::a_plan_breaking_protection_is_refused_and_kept,
            check::a_batch_not_prepared_is_refused,
            check::an_unchanged_duplicate_target_wins_by_seam_repository,
            selection::nothing_changed_selects_nothing,
            selection::each_change_alone_selects_exactly_its_ticket,
            selection::full_selects_every_open_ticket,
            selection::overlap_comments_select_by_what_the_record_holds,
            selection::an_older_record_holds_the_comment_read_back,
            selection::a_record_this_does_not_read_selects_its_ticket,
        );
    };
    (@each $runner:expr; $($module:ident :: $journey:ident),* $(,)?) => {
        $(
            #[test]
            fn $journey() {
                $crate::groom::$module::$journey(&$runner);
            }
        )*
    };
}

pub(crate) use groom_journeys;
