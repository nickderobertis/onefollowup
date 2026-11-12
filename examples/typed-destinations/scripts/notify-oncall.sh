#!/bin/sh
# The `page-on-critical` route's command: page this week's on-call about a critical bug, then
# assign the bug to them.
#
# stdin: the route command's document, {"schema_version": 1, "ticket", "kind", "severity",
# "repository", "paths", "route"}, which is handed to the pager as it is.
# stdout: {"assignee": "<the on-call's display name, as the bug tracker names people>"}.
# A non-zero exit withholds the bug's write, receipted `route-command-failed`, and a retry
# pages again. `oncall` is the team's paging CLI; `ONCALL_SCHEDULE` names its rotation.
set -eu
schedule="${ONCALL_SCHEDULE:-bugs}"
oncall page --schedule "$schedule" --title "critical bug filed" --stdin
person="$(oncall who --schedule "$schedule")"
printf '{"assignee": "%s"}\n' "$person"
