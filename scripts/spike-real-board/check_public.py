"""Fail unless every issue and repository the spike's files name is one GitHub reports public.

    python3 check_public.py [PATH...] [--fresh] [--names]

Scans every file under PATH (default: this directory, which holds every recorded request,
response, snapshot, summary and fixture), gzip-compressed ones decompressed, tracked or not,
but the filter's unit test, whose names are invented (`INVENTED`).
It gathers every repository the files name and every issue, board item or project they name
by node id (`public.names_in`: a `nameWithOwner` or `repository` value, a github.com URL, a
cross-repository issue reference), and asks GitHub for each: a repository passes only when GitHub
answers `visibility: PUBLIC`; a node id only when it is an issue of such a repository, a board
item of the `followups` board whose content is such an issue, or that board itself (GitHub
reports the board as not public; only its items whose issues are public are kept).
An issue named by URL or reference is public exactly when its repository is. Anything GitHub
does not answer for fails. Exit 0 prints what was checked; exit 1 prints, per file, how many
names failed (`--names` prints the names themselves, for a person's terminal only).

Verdicts for public names are cached under `$XDG_CACHE_HOME/onefollowup-spike-real-board`
(`--fresh` ignores the cache); a non-public name is never written anywhere.
"""

from __future__ import annotations

import gzip
import json
import os
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

# E402: run as a script from anywhere, this file's directory goes on the path first.
import ghq  # noqa: E402
import public  # noqa: E402

# The filter's own unit test: every repository and id in it is invented to exercise the
# filter, names nothing on GitHub, and is recorded from nowhere.
INVENTED = {"test_public.py"}

CACHE = (
    Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "onefollowup-spike-real-board"
)


def text_of(path: Path) -> str:
    raw = path.read_bytes()
    if path.suffix == ".gz":
        try:
            raw = gzip.decompress(raw)
        except EOFError:
            # A stream a stopped process left unterminated: read what it flushed.
            with gzip.open(path) as held:
                chunks = []
                try:
                    while chunk := held.read(1 << 16):
                        chunks.append(chunk)
                except EOFError:
                    pass
                raw = b"".join(chunks)
    return raw.decode("utf-8", errors="replace")


def cached(kind: str, fresh: bool) -> set[str]:
    path = CACHE / f"public-{kind}.txt"
    if fresh or not path.exists():
        return set()
    return {line for line in path.read_text().splitlines() if line}


def remember(kind: str, names: set[str]) -> None:
    CACHE.mkdir(parents=True, exist_ok=True)
    (CACHE / f"public-{kind}.txt").write_text("".join(n + "\n" for n in sorted(names)))


def public_repositories(names: set[str], known: set[str]) -> set[str]:
    # This check writes nothing into the recordings: no cache of the filter's own.
    visibility = public.Visibility(cache=False, known=known)
    visibility.resolve(names)
    return {name for name in names if visibility.answers.get(name)}


def main() -> int:
    args = [one for one in sys.argv[1:] if not one.startswith("--")]
    fresh, show = "--fresh" in sys.argv, "--names" in sys.argv
    roots = [Path(one) for one in args] or [HERE]
    files = sorted(
        path
        for root in roots
        for path in ([root] if root.is_file() else root.rglob("*"))
        if path.is_file() and "__pycache__" not in path.parts and path.name not in INVENTED
    )
    by_file: dict[Path, tuple[set[str], set[str]]] = {}
    for path in files:
        by_file[path] = public.names_in(text_of(path))
    repositories = set().union(*(held[0] for held in by_file.values())) if by_file else set()
    ids = set().union(*(held[1] for held in by_file.values())) if by_file else set()
    spent_before = ghq.spent()
    passed_repositories = public_repositories(
        repositories, cached("repositories", fresh) & repositories
    )
    board_id = public.board_project_id() if ids else ""
    known_ids = cached("nodes", fresh) & ids
    passed_ids = known_ids | public.public_nodes(
        ids - known_ids, board_id, "public check: node ids"
    )
    remember("repositories", cached("repositories", False) | passed_repositories)
    remember("nodes", cached("nodes", False) | passed_ids)
    failures: dict[Path, list[str]] = defaultdict(list)
    for path, (named, nodes) in by_file.items():
        failures[path].extend(sorted(named - passed_repositories))
        failures[path].extend(sorted(nodes - passed_ids))
    failing = {path: names for path, names in failures.items() if names}
    report = {
        "files_scanned": len(files),
        "repositories_named": len(repositories),
        "repositories_public": len(passed_repositories),
        "node_ids_named": len(ids),
        "node_ids_public": len(ids & passed_ids),
        "points_this_check_spent": ghq.spent() - spent_before,
    }
    if failing:
        report["failing_files"] = {
            str(
                path.relative_to(HERE.parent.parent)
                if path.is_relative_to(HERE.parent.parent)
                else path
            ): (names if show else len(names))
            for path, names in failing.items()
        }
        print(json.dumps(report, indent=2))
        print(
            "check_public: these files name an issue or repository GitHub does not report as "
            "public; drop it from them (rerun with --names on your own terminal to see which)",
            file=sys.stderr,
        )
        return 1
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
