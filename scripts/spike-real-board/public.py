"""What this spike may record: content only from issues whose repository GitHub reports as
public. Every writer of a file under `recordings/` passes what it writes through `Filter`.

A repository is public only when GitHub answers `visibility: PUBLIC` for it; one it does not
answer for, or answers anything else for, is not, and nothing of it is kept. Only public
repository names are ever persisted (`recordings/public-repositories.txt`, a cache that saves
the next process a query); a non-public name is held in memory only and never written.

`Filter.scrub(value)` returns `value` with every item whose repository is not public dropped —
an issue node, a board item whose content is such an issue, a pull request or draft issue (no
repository to vouch for it), a prepare candidate whose URL names such a repository — and every
other project's board membership reduced to the fact of it. `Filter.dropped` counts what it
dropped; nothing else of a dropped item survives. `Filter.scrub_variables` replaces every node
id a recorded request names that is not a kept item's with `"(dropped)"`.
"""

from __future__ import annotations

import json
import re
import threading

import ghq

CACHE = ghq.RECORDINGS / "public-repositories.txt"
BOARD_OWNER = "nickderobertis"
BOARD_NUMBER = 3
DROPPED = "(dropped)"
# A GitHub global node id: an issue's (`I_`), a board item's (`PVTI_`), a project's (`PVT_`),
# a field's or option's; a recorded variable holding one names an item.
NODE_ID = re.compile(r"^(I|PVTI|PVT|PVTSSF|PVTF|PR|DI|IC|LA|MDU|U|R)_[A-Za-z0-9_-]{6,}$")
NODE_ID_IN_TEXT = re.compile(r"(?<![\w-])(?:I|PVTI|PVT|PR|DI)_[A-Za-z0-9_-]{6,}")
ISSUE_URL = re.compile(r"https://github\.com/([\w.-]+/[\w.-]+)/(?:issues|pull)/\d+")
# How a file names a repository: a `nameWithOwner` or `repository` value, a github.com URL, a
# cross-repository reference (owner, slash, repository, hash, number).
NAMED = re.compile(r'"(?:nameWithOwner|repository)"\s*:\s*"([A-Za-z0-9-]+/[A-Za-z0-9._-]+)"')
URL = re.compile(r"(?<![\w.-])github\.com/([A-Za-z0-9-]+)/([A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*)")
REFERENCE = re.compile(r"(?<![\w./-])([A-Za-z0-9-]+/[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*)#\d+\b")
# First path segments of github.com that are GitHub's own pages, not an owner.
RESERVED = {
    "about",
    "account",
    "advisories",
    "apps",
    "codespaces",
    "collections",
    "contact",
    "enterprise",
    "explore",
    "features",
    "issues",
    "login",
    "marketplace",
    "new",
    "notifications",
    "organizations",
    "orgs",
    "pricing",
    "pulls",
    "search",
    "security",
    "settings",
    "site",
    "sponsors",
    "topics",
    "trending",
    "users",
}


def names_in(text: str) -> tuple[set[str], set[str]]:
    """The repositories and the node ids `text` names."""
    repositories = set(NAMED.findall(text))
    for owner, repository in URL.findall(text):
        if owner.lower() not in RESERVED:
            repositories.add(f"{owner}/{repository.removesuffix('.git')}")
    repositories.update(REFERENCE.findall(text))
    return repositories, set(NODE_ID_IN_TEXT.findall(text))


def public_nodes(ids: set[str], board_id: str, purpose: str) -> set[str]:
    """Those of `ids` that are an issue of a public repository, an item of the board whose
    content is one, or the board itself (about 1 point a 100)."""
    passed: set[str] = set()
    unknown = sorted(ids)
    for at in range(0, len(unknown), 100):
        chunk = unknown[at : at + 100]
        answer = ghq.graphql(
            "query($ids:[ID!]!){actualCost:rateLimit{cost} nodes(ids:$ids){__typename id "
            "... on Issue{repository{visibility}} "
            "... on ProjectV2Item{project{id} content{__typename ... on Issue{repository{visibility}}}}}}",
            {"ids": chunk},
            purpose=purpose,
            tolerate_errors=True,
        )
        for node in (answer.get("data") or {}).get("nodes") or []:
            if not node:
                continue
            kind = node.get("__typename")
            if kind == "Issue":
                ok = (node.get("repository") or {}).get("visibility") == "PUBLIC"
            elif kind == "ProjectV2Item":
                content = node.get("content") or {}
                ok = (
                    (node.get("project") or {}).get("id") == board_id
                    and content.get("__typename") == "Issue"
                    and (content.get("repository") or {}).get("visibility") == "PUBLIC"
                )
            elif kind == "ProjectV2":
                ok = node.get("id") == board_id
            else:
                ok = False
            if ok:
                passed.add(node["id"])
    return passed


class Visibility:
    """GitHub's answer to "is this repository public?", asked once a name per process."""

    def __init__(self, *, cache: bool = True, known: set[str] = frozenset()) -> None:
        self.answers: dict[str, bool] = {name: True for name in known}
        self.lock = threading.Lock()
        self.cache = cache
        # Offline, a name not yet answered for is not public, and nothing is asked.
        self.offline = False
        if cache and CACHE.exists():
            for line in CACHE.read_text().splitlines():
                if line.strip():
                    self.answers[line.strip()] = True

    def resolve(self, names: set[str]) -> None:
        if self.offline:
            return
        with self.lock:
            unknown = sorted(name for name in names if name not in self.answers)
        for at in range(0, len(unknown), 50):
            chunk = unknown[at : at + 50]
            slots, variables, heads = [], {}, []
            for n, name in enumerate(chunk):
                owner, _, repository = name.partition("/")
                heads.append(f"$o{n}:String!,$n{n}:String!")
                slots.append(f"r{n}:repository(owner:$o{n},name:$n{n}){{nameWithOwner visibility}}")
                variables[f"o{n}"], variables[f"n{n}"] = owner, repository
            document = f"query({','.join(heads)}){{actualCost:rateLimit{{cost}} {' '.join(slots)}}}"
            answer = ghq.graphql(
                document, variables, purpose="repository visibility", tolerate_errors=True
            )
            data = answer.get("data") or {}
            with self.lock:
                for n, name in enumerate(chunk):
                    held = data.get(f"r{n}") or {}
                    self.answers[name] = held.get("visibility") == "PUBLIC"
            self.persist()

    def preload(self, owner: str) -> None:
        """Every repository of `owner` the token can see, in one walk, so the run's own
        answers need no query of their own (about 1 point a 100)."""
        after = None
        while True:
            answer = ghq.graphql(
                "query($o:String!,$after:String){actualCost:rateLimit{cost} "
                "repositoryOwner(login:$o){repositories(first:100,after:$after){"
                "pageInfo{hasNextPage endCursor} nodes{nameWithOwner visibility}}}}",
                {"o": owner, "after": after},
                purpose="repository visibility of the board's owner",
            )
            connection = answer["data"]["repositoryOwner"]["repositories"]
            with self.lock:
                for node in connection["nodes"]:
                    self.answers[node["nameWithOwner"]] = node["visibility"] == "PUBLIC"
            if not connection["pageInfo"]["hasNextPage"]:
                break
            after = connection["pageInfo"]["endCursor"]
        self.persist()

    def persist(self) -> None:
        if not self.cache:
            return
        with self.lock:
            public = sorted(name for name, yes in self.answers.items() if yes)
            CACHE.parent.mkdir(parents=True, exist_ok=True)
            CACHE.write_text("".join(name + "\n" for name in public))

    def public(self, name: str | None) -> bool:
        if not name:
            return False
        self.resolve({name})
        return self.answers.get(name, False)


def repositories_in(value, found: set[str]) -> set[str]:
    """Every repository `value` names by `nameWithOwner` or an issue URL."""
    if isinstance(value, dict):
        repository = value.get("repository")
        if isinstance(repository, dict) and repository.get("nameWithOwner"):
            found.add(repository["nameWithOwner"])
        for held in value.values():
            repositories_in(held, found)
    elif isinstance(value, list):
        for held in value:
            repositories_in(held, found)
    elif isinstance(value, str):
        for match in ISSUE_URL.finditer(value):
            found.add(match.group(1))
    return found


class Filter:
    def __init__(
        self,
        visibility: Visibility | None = None,
        board_project_id: str | None = None,
        resolve_nodes=None,
    ):
        self.visibility = visibility or Visibility()
        # GitHub's answer for node ids: `public_nodes`, or a stand-in where no token is.
        self.resolve_nodes = resolve_nodes or public_nodes
        self.board_project_id = board_project_id
        self.dropped = 0
        self.mentions_dropped = 0
        self.kept_ids: set[str] = set()
        self.public_ids: set[str] = set()
        self.lock = threading.Lock()

    def _drop(self) -> None:
        with self.lock:
            self.dropped += 1

    def _issue_public(self, value: dict) -> bool:
        repository = value.get("repository")
        name = repository.get("nameWithOwner") if isinstance(repository, dict) else None
        return self.visibility.public(name)

    def _verdict(self, value: dict) -> bool | None:
        """False to drop `value`, True to keep it as an item, None when it is not an item."""
        kind = value.get("__typename")
        if kind == "Issue":
            return self._issue_public(value)
        if kind in ("PullRequest", "DraftIssue"):
            return False
        if "content" in value and ("fieldValues" in value or "type" in value):
            content = value.get("content")
            if not isinstance(content, dict) or not content:
                return False
            return content.get("__typename") == "Issue" and self._issue_public(content)
        url = value.get("url")
        if isinstance(url, str) and "item" in value and "title" in value:
            match = ISSUE_URL.match(url)
            return bool(match) and self.visibility.public(match.group(1))
        return None

    def scrub(self, value):
        self.visibility.resolve(repositories_in(value, set()))
        return self.scrub_mentions(self._scrub(value))

    def scrub_mentions(self, value):
        """`value` with every mention, inside a kept item's text, of a repository or node id
        GitHub does not report as public replaced by `"(dropped)"`."""
        text = json.dumps(value)
        repositories, ids = names_in(text)
        self.visibility.resolve(repositories)
        unknown = ids - self.kept_ids - self.public_ids
        if unknown:
            self.public_ids |= self.resolve_nodes(
                unknown, self.board_project_id or "", "public filter: node ids"
            )
        bad_repositories = {name for name in repositories if not self.visibility.public(name)}
        bad_ids = ids - self.kept_ids - self.public_ids
        if not bad_repositories and not bad_ids:
            return value

        def walk(held):
            if isinstance(held, dict):
                return {key: walk(one) for key, one in held.items()}
            if isinstance(held, list):
                return [walk(one) for one in held]
            if isinstance(held, str):
                return self._mention(held, bad_repositories, bad_ids)
            return held

        return walk(value)

    def _mention(self, text: str, bad_repositories: set[str], bad_ids: set[str]) -> str:
        for name in sorted(bad_repositories, key=len, reverse=True):
            if name in text:
                with self.lock:
                    self.mentions_dropped += text.count(name)
                text = text.replace(name, DROPPED)
        for one in sorted(bad_ids, key=len, reverse=True):
            if one in text:
                with self.lock:
                    self.mentions_dropped += text.count(one)
                text = text.replace(one, DROPPED)
        return text

    def _scrub(self, value):
        if isinstance(value, list):
            kept = []
            for held in value:
                if isinstance(held, dict) and self._verdict(held) is False:
                    self._drop()
                    continue
                kept.append(self._scrub(held))
            return kept
        if not isinstance(value, dict):
            return value
        verdict = self._verdict(value)
        if verdict is False:
            self._drop()
            return None
        if verdict is True:
            for key in ("id",):
                if isinstance(value.get(key), str):
                    self.kept_ids.add(value[key])
            content = value.get("content")
            if isinstance(content, dict) and isinstance(content.get("id"), str):
                self.kept_ids.add(content["id"])
        out = {}
        for key, held in value.items():
            if key == "projectItems" and isinstance(held, dict):
                out[key] = self._memberships(held)
            else:
                out[key] = self._scrub(held)
        return out

    def _memberships(self, connection: dict) -> dict:
        """An issue's board memberships, every project but the board reduced to the fact."""
        nodes = []
        for node in connection.get("nodes") or []:
            project = (node or {}).get("project") or {}
            if self.board_project_id and project.get("id") == self.board_project_id:
                nodes.append(self._scrub(node))
                if isinstance(node.get("id"), str):
                    self.kept_ids.add(node["id"])
            else:
                nodes.append({"other_project": True})
        return {**{k: v for k, v in connection.items() if k != "nodes"}, "nodes": nodes}

    def scrub_text(self, text: str) -> str:
        """`text` with every node id not a kept item's, and every issue URL of a repository
        that is not public, replaced by `"(dropped)"`."""
        self.visibility.resolve({m.group(1) for m in ISSUE_URL.finditer(text)})

        def node(match: re.Match) -> str:
            if match.group(0) in self.kept_ids:
                return match.group(0)
            self._drop()
            return DROPPED

        def url(match: re.Match) -> str:
            if self.visibility.public(match.group(1)):
                return match.group(0)
            self._drop()
            return DROPPED

        return ISSUE_URL.sub(url, NODE_ID_IN_TEXT.sub(node, text))

    def scrub_variables(self, variables: dict) -> dict:
        out = {}
        for key, held in variables.items():
            if isinstance(held, str) and NODE_ID.match(held) and held not in self.kept_ids:
                out[key] = DROPPED
            else:
                out[key] = held
        return out


def board_project_id() -> str:
    """The `followups` board's own project id (1 point). GitHub reports the board itself as
    not public; what the spike keeps of it is only its items whose issues are public."""
    answer = ghq.graphql(
        "query($o:String!,$n:Int!){actualCost:rateLimit{cost} "
        "repositoryOwner(login:$o){... on ProjectV2Owner{projectV2(number:$n){id public}}}}",
        {"o": BOARD_OWNER, "n": BOARD_NUMBER},
        purpose="board project id",
    )
    return answer["data"]["repositoryOwner"]["projectV2"]["id"]
