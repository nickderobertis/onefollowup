#!/bin/sh
# The `batch_preparing` hook: each budget overrun the repository's budget step recorded becomes
# one more input of the batch, which the verifying agent disposes like a draft.
#
# stdin: {"schema_version": 1, "batch": "<id>"} (read and ignored: every overrun recorded is
# this batch's). The overruns file, `$FOLLOW_UP_BUDGET_OVERRUNS` or
# `target/budget-results/overruns.json`, is a JSON array of
# {"id", "source", "summary", "detail"}, written by the budget step; absent, there are none.
# stdout: {"schema_version": 1, "inputs": [...]}.
set -eu
cat >/dev/null
overruns="${FOLLOW_UP_BUDGET_OVERRUNS:-target/budget-results/overruns.json}"
if [ -s "$overruns" ]; then
  printf '{"schema_version": 1, "inputs": '
  cat "$overruns"
  printf '}\n'
else
  printf '{"schema_version": 1, "inputs": []}\n'
fi
