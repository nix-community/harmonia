#!/usr/bin/env python3

"""Generate a Mermaid dependency diagram of intra-workspace crate dependencies.

Produces the transitive reduction of the dependency graph.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
ROOT = SCRIPT_DIR.parent
DEFAULT_DOC_FILE = ROOT / "docs" / "architecture" / "harmonia-store-structure.md"
PREFIX = "harmonia-"


def short(name: str) -> str:
    """Strip the ``harmonia-`` prefix for readable node labels."""
    return name.removeprefix(PREFIX)


def get_workspace_info(
    manifest_path: str | None = None,
) -> tuple[list[str], list[tuple[str, str]]]:
    """Return (all member names, edge pairs) for intra-workspace deps."""
    cmd = ["cargo", "metadata", "--format-version=1", "--no-deps"]
    if manifest_path:
        cmd += ["--manifest-path", manifest_path]
    raw = subprocess.check_output(cmd, text=True)
    meta = json.loads(raw)
    members = {p["name"] for p in meta["packages"]}

    all_members = sorted(short(name) for name in members)

    edges: list[tuple[str, str]] = []
    for pkg in meta["packages"]:
        if pkg["name"] not in members:
            continue
        for dep in pkg["dependencies"]:
            if dep["name"] not in members:
                continue
            # Skip dev / build deps
            if dep.get("kind") not in (None, "normal"):
                continue
            # Skip optional (feature-gated) deps
            if dep.get("optional", False):
                continue
            edges.append((short(pkg["name"]), short(dep["name"])))

    return all_members, sorted(set(edges))


def transitive_reduction(
    edges: list[tuple[str, str]],
) -> list[tuple[str, str]]:
    """Compute the transitive reduction (Hasse diagram) of a DAG."""
    # Build adjacency: src -> set of direct successors (dependencies).
    adj: dict[str, set[str]] = {}
    for src, dst in edges:
        adj.setdefault(src, set()).add(dst)
        adj.setdefault(dst, set())

    # For each node, compute the full set of reachable nodes.
    reachable: dict[str, set[str]] = {}

    def reach(n: str) -> set[str]:
        if n in reachable:
            return reachable[n]
        r: set[str] = set()
        for child in adj[n]:
            r.add(child)
            r |= reach(child)
        reachable[n] = r
        return r

    for n in adj:
        reach(n)

    # An edge src→dst is redundant if dst is reachable from src
    # through some other direct successor.
    reduced: list[tuple[str, str]] = []
    for src, dst in edges:
        others = adj[src] - {dst}
        reachable_without = set()
        for o in others:
            reachable_without.add(o)
            reachable_without |= reachable[o]
        if dst not in reachable_without:
            reduced.append((src, dst))

    return sorted(reduced)


def topo_order(edges: list[tuple[str, str]]) -> dict[str, int]:
    """Return a topological rank for each node (0 = leaf dependency)."""
    nodes: set[str] = set()
    for src, dst in edges:
        nodes.add(src)
        nodes.add(dst)

    children: dict[str, set[str]] = {n: set() for n in nodes}
    in_degree: dict[str, int] = {n: 0 for n in nodes}
    for src, dst in edges:
        children[dst].add(src)
        in_degree[src] += 1

    order: dict[str, int] = {}
    queue = sorted(n for n in nodes if in_degree[n] == 0)
    rank = 0
    while queue:
        next_queue: list[str] = []
        for n in queue:
            order[n] = rank
        for n in queue:
            for child in children[n]:
                in_degree[child] -= 1
                if in_degree[child] == 0:
                    next_queue.append(child)
        queue = sorted(next_queue)
        rank += 1

    return order


def generate_mermaid(
    all_members: list[str],
    edges: list[tuple[str, str]],
    title: str | None = None,
) -> str:
    nodes: set[str] = set(all_members)
    for src, dst in edges:
        nodes.add(src)
        nodes.add(dst)

    order = topo_order(edges)

    # Store crates that perform I/O (blacklist — excluded from the pure group).
    STORE_IMPURE = {
        "store-db",
        "store-remote",
    }
    # Store crates that are I/O-free (whitelist — included in the pure group).
    STORE_PURE = {
        "store-aterm",
        "store-build-result",
        "store-content-address",
        "store-derivation",
        "store-nar-info",
        "store-path",
        "store-path-info",
    }

    # Sanity check: every store-* crate must be in exactly one list.
    overlap = STORE_PURE & STORE_IMPURE
    if overlap:
        raise SystemExit(
            f"Store crate(s) in both STORE_PURE and STORE_IMPURE: {', '.join(sorted(overlap))}. "
            f"Each crate must be in exactly one set."
        )
    store_crates = {n for n in nodes if n.startswith("store-")}
    uncategorized = store_crates - STORE_PURE - STORE_IMPURE
    if uncategorized:
        raise SystemExit(
            f"Store crate(s) not categorized: {', '.join(sorted(uncategorized))}. "
            f"Add to STORE_PURE or STORE_IMPURE in {__file__}."
        )

    # Group crates by prefix so Mermaid clusters them.
    groups = {
        "Utilities": sorted(n for n in nodes if n.startswith("utils-")),
        "Store (pure)": sorted(n for n in nodes if n in STORE_PURE),
        "File": sorted(n for n in nodes if n.startswith("file-")),
    }
    grouped = {n for members in groups.values() for n in members}

    lines = ["```mermaid"]
    if title:
        lines.append("---")
        lines.append(f"title: {title}")
        lines.append("---")
    lines.append("graph BT")
    for label, members in groups.items():
        if members:
            lines.append(f'    subgraph "{label}"')
            for n in members:
                lines.append(f"        {n}")
            lines.append("    end")

    # Emit isolated nodes (no edges) that aren't in a subgraph.
    connected = set()
    for src, dst in edges:
        connected.add(src)
        connected.add(dst)
    for n in sorted(nodes - connected - grouped):
        lines.append(f"    {n}")

    sorted_edges = sorted(edges, key=lambda e: (order.get(e[0], 0), e[0], e[1]))
    for src, dst in sorted_edges:
        lines.append(f"    {src} --> {dst}")
    lines.append("```")
    return "\n".join(lines)


def main() -> None:
    manifest_path = None
    if "--manifest-path" in sys.argv:
        idx = sys.argv.index("--manifest-path")
        manifest_path = sys.argv[idx + 1]

    all_members, edges = get_workspace_info(manifest_path)
    reduced = transitive_reduction(edges)

    mermaid = generate_mermaid(all_members, reduced, title="Transitive reduction")

    # --doc PATH: specify the doc file (default: auto-detected from script location).
    if "--doc" in sys.argv:
        idx = sys.argv.index("--doc")
        doc_file = Path(sys.argv[idx + 1])
    else:
        doc_file = DEFAULT_DOC_FILE

    if doc_file.exists():
        text = doc_file.read_text()
        blocks = list(re.finditer(r"```mermaid\n.*?```", text, flags=re.DOTALL))
        if blocks:
            text = text[: blocks[0].start()] + mermaid + text[blocks[-1].end() :]
        else:
            print("No mermaid blocks found in doc file", file=sys.stderr)
            sys.exit(1)

        if "--update" in sys.argv:
            doc_file.write_text(text)
            print(f"Updated {doc_file}", file=sys.stderr)
        else:
            print(text, end="")
    else:
        print(mermaid)


if __name__ == "__main__":
    main()
