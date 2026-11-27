"""The public filter keeps only what GitHub reports public: `python3 -m unittest test_public`.

GitHub's visibility answers are given in memory (no token here); the filter itself runs as
the forwarder, the snapshot and `outputs.py` run it.
"""

from __future__ import annotations

import json
import unittest

import public

BOARD = "PVT_board00001"


def gate() -> public.Filter:
    visibility = public.Visibility(cache=False, known={"owner/open-repo"})
    visibility.answers["owner/closed-repo"] = False
    return public.Filter(
        visibility,
        board_project_id=BOARD,
        resolve_nodes=lambda ids, board, purpose: {one for one in ids if "PUB" in one},
    )


def issue(id_: str, repository: str, **extra) -> dict:
    return {"__typename": "Issue", "id": id_, "repository": {"nameWithOwner": repository}, **extra}


class FilterTest(unittest.TestCase):
    def test_drops_every_kind_of_item_whose_repository_is_not_public(self):
        f = gate()
        answer = {
            "data": {
                "s0": {
                    "nodes": [
                        issue("I_kwPUB000001", "owner/open-repo", title="kept"),
                        issue("I_kwHIDE00001", "owner/closed-repo", title="secret title"),
                        {"__typename": "DraftIssue", "id": "DI_x000001", "title": "draft"},
                    ]
                },
                "i0": issue("I_kwHIDE00001", "owner/closed-repo", body="secret body"),
                "o0": {
                    "items": {
                        "nodes": [
                            {
                                "id": "PVTI_hide0001",
                                "fieldValues": {},
                                "content": issue("I_kwHIDE00002", "owner/closed-repo"),
                            },
                        ]
                    }
                },
            }
        }
        kept = f.scrub(answer)
        text = json.dumps(kept)
        self.assertNotIn("secret", text)
        self.assertNotIn("closed-repo", text)
        self.assertEqual([n["id"] for n in kept["data"]["s0"]["nodes"]], ["I_kwPUB000001"])
        self.assertIsNone(kept["data"]["i0"])
        self.assertEqual(kept["data"]["o0"]["items"]["nodes"], [])
        self.assertEqual(f.dropped, 4)

    def test_unknown_repository_is_not_public(self):
        f = gate()
        f.visibility.answers["owner/never-answered"] = False
        kept = f.scrub([issue("I_kwHIDE00003", "owner/never-answered"), issue("I_kw000004", "")])
        self.assertEqual(kept, [])

    def test_other_projects_memberships_keep_only_the_fact(self):
        f = gate()
        node = issue(
            "I_kwPUB000002",
            "owner/open-repo",
            projectItems={
                "nodes": [
                    {"id": "PVTI_PUB00001", "project": {"id": BOARD}},
                    {
                        "id": "PVTI_other001",
                        "project": {"id": "PVT_other0001"},
                        "fieldValues": {"nodes": [{"text": "secret"}]},
                    },
                ]
            },
        )
        kept = f.scrub([node])[0]
        self.assertEqual(kept["projectItems"]["nodes"][1], {"other_project": True})
        self.assertNotIn("secret", json.dumps(kept))

    def test_mentions_in_public_text_of_non_public_names_are_dropped(self):
        f = gate()
        body = "see owner/closed-repo#3, https://github.com/owner/closed-repo/issues/4 and I_kwHIDE00009; owner/open-repo#1 stays"
        kept = f.scrub([issue("I_kwPUB000003", "owner/open-repo", body=body)])[0]
        self.assertNotIn("closed-repo", kept["body"])
        self.assertNotIn("I_kwHIDE00009", kept["body"])
        self.assertIn("owner/open-repo#1", kept["body"])
        self.assertEqual(f.mentions_dropped, 3)

    def test_variables_and_progress_lines_name_only_kept_ids(self):
        f = gate()
        f.scrub([issue("I_kwPUB000004", "owner/open-repo")])
        self.assertEqual(
            f.scrub_variables({"id0": "I_kwPUB000004", "id1": "I_kwHIDE00005", "search0": "terms"}),
            {"id0": "I_kwPUB000004", "id1": "(dropped)", "search0": "terms"},
        )
        line = "started: 2 items (followups:I_kwPUB000004, followups:I_kwHIDE00005)"
        self.assertEqual(
            f.scrub_text(line), "started: 2 items (followups:I_kwPUB000004, followups:(dropped))"
        )

    def test_prepare_candidates_of_non_public_repositories_are_dropped(self):
        f = gate()
        kept = f.scrub(
            {
                "candidates": [
                    {
                        "item": "followups:I_kwHIDE00006",
                        "title": "secret",
                        "url": "https://github.com/owner/closed-repo/issues/9",
                    },
                    {
                        "item": "followups:I_kwPUB000005",
                        "title": "fine",
                        "url": "https://github.com/owner/open-repo/issues/2",
                    },
                ]
            }
        )
        self.assertEqual([c["title"] for c in kept["candidates"]], ["fine"])


if __name__ == "__main__":
    unittest.main()
