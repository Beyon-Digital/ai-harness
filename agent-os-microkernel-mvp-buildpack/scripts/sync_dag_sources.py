#!/usr/bin/env python3
"""Regenerate dag.json, DAG.md, and tasks.csv from dag.yaml.

dag.yaml is the single hand-editable machine-readable source. This script
derives the other three sources so they cannot drift apart:

- dag.json: the parsed task graph, two-space JSON, ensure_ascii=False,
  trailing newline.
- DAG.md: the mermaid graph plus the numbered topological execution order;
  the order is Kahn's algorithm with a min-heap keyed by task ID.
- tasks.csv: columns id,phase,title,depends_on with dependencies joined by a
  single space.

Usage:
    python3 scripts/sync_dag_sources.py          # write derived sources
    python3 scripts/sync_dag_sources.py --check  # exit 2 if any output differs
"""

from __future__ import annotations

import argparse
import csv
import heapq
import io
import json
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]
DAG_YAML = ROOT / "dag.yaml"
DAG_JSON = ROOT / "dag.json"
DAG_MD = ROOT / "DAG.md"
TASKS_CSV = ROOT / "tasks.csv"

PARALLELISM_RULE = (
    "Tasks at the same dependency frontier may run concurrently only when "
    "they do not edit the same code ownership boundary. Use independent Git "
    "worktrees for parallel coding agents; merge after each branch passes its "
    "task tests."
)


def load_graph() -> dict:
    with DAG_YAML.open(encoding="utf-8") as handle:
        return yaml.safe_load(handle)


def render_json(graph: dict) -> str:
    return json.dumps(graph, indent=2, ensure_ascii=False) + "\n"


def mermaid_id(task_id: str) -> str:
    return task_id.replace("-", "_")


def render_dag_md(graph: dict) -> str:
    tasks = graph["tasks"]
    lines = [
        "# Implementation DAG",
        "",
        f"**{len(tasks)} tasks**, acyclic. Machine-readable source: [`dag.yaml`](dag.yaml).",
        "",
        "## Mermaid graph",
        "",
        "```mermaid",
        "flowchart TD",
    ]
    for task in tasks:
        lines.append(f'  {mermaid_id(task["id"])}["{task["id"]}: {task["title"]}"]')
    for task in tasks:
        for dep in task["depends_on"]:
            lines.append(f"  {mermaid_id(dep)} --> {mermaid_id(task['id'])}")
    lines.append("```")
    lines.extend(
        [
            "",
            "## Valid topological execution order",
            "",
        ]
    )
    for index, task in enumerate(topological_order(graph), start=1):
        lines.append(f"{index}. [{task['id']} — {task['title']}](tasks/{task['id']}.md)")
    lines.extend(
        [
            "",
            "## Parallelism rule",
            "",
            PARALLELISM_RULE,
            "",
        ]
    )
    return "\n".join(lines)


def topological_order(graph: dict) -> list[dict]:
    tasks = graph["tasks"]
    by_id = {task["id"]: task for task in tasks}
    indegree = {task["id"]: len(task["depends_on"]) for task in tasks}
    children: dict[str, list[str]] = {task["id"]: [] for task in tasks}
    for task in tasks:
        for dep in task["depends_on"]:
            children[dep].append(task["id"])
    ready = [task_id for task_id in indegree if indegree[task_id] == 0]
    heapq.heapify(ready)
    order: list[dict] = []
    while ready:
        current = heapq.heappop(ready)
        order.append(by_id[current])
        for child in children[current]:
            indegree[child] -= 1
            if indegree[child] == 0:
                heapq.heappush(ready, child)
    if len(order) != len(tasks):
        raise SystemExit("dag.yaml contains a cycle; refusing to generate sources")
    return order


def render_tasks_csv(graph: dict) -> str:
    buffer = io.StringIO()
    writer = csv.writer(buffer, lineterminator="\r\n")
    writer.writerow(["id", "phase", "title", "depends_on"])
    for task in graph["tasks"]:
        writer.writerow(
            [
                task["id"],
                task["phase"],
                task["title"],
                " ".join(task["depends_on"]),
            ]
        )
    return buffer.getvalue()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 2 if any derived source differs from a fresh generation",
    )
    args = parser.parse_args()

    graph = load_graph()
    outputs = {
        DAG_JSON: render_json(graph),
        DAG_MD: render_dag_md(graph),
        TASKS_CSV: render_tasks_csv(graph),
    }

    if args.check:
        drifted = []
        for path, rendered in outputs.items():
            current = path.read_bytes() if path.exists() else None
            if current != rendered.encode("utf-8"):
                drifted.append(path)
        if drifted:
            for path in drifted:
                print(f"out of date: {path.relative_to(ROOT)}")
            print("run: python3 scripts/sync_dag_sources.py")
            return 2
        print("DAG sources up to date")
        return 0

    for path, rendered in outputs.items():
        path.write_bytes(rendered.encode("utf-8"))
        print(f"wrote {path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
