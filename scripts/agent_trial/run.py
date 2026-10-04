#!/usr/bin/env python3
"""Agent trial (plan step 5.4): run each task with and without the annatar CLI.

Usage:
  run.py run --tasks tasks.toml --out DIR [--reps 2] [--only T1,T2] [--arms with,without]
  run.py summary --out DIR          # per-run CSV + per task/arm means; warns about incomplete runs
  run.py blind --out DIR            # shuffled final answers for grading, annatar mentions redacted

`run` writes DIR/raw/<task>-<arm>-<rep>.jsonl (the stream) and .meta.json (exit code, timeout,
wall time); a run without a result event is repeated by the next `run`.
Tests (stdlib, no network): python3 -m unittest discover -s scripts/agent_trial

The task file (private) holds `preamble` and `[[task]]` tables with `id`, `label`, `prompt`.
Environment: ANNATAR_TRIAL_REPO (cwd of the agent), ANNATAR_TRIAL_BIN (directory with an
`annatar` wrapper, put on PATH for the "with" arm only), ANNATAR_TRIAL_MODEL.
"""

import argparse
import csv
import json
import os
import random
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import time
import tomllib
from pathlib import Path

WITH_PROMPT = """This machine has `annatar`, a command-line index of this repository. Every class, interface, method and constructor has a short description of what it does and, where Jira tickets or commits explain it, why it was built or changed.
- `annatar search "<words>"` lists the symbols whose descriptions are most similar in meaning to the words: rank, similarity, fully qualified name (third field), [kind], role, file:lines, and the description on the next line. `-k N` sets the number of results (default 10).
- `annatar show '<fqn>'` prints one symbol (fqn exactly as search prints it): description, parent, children, file and lines, recent commits and Jira tickets with an English summary and purpose.
Results are ranked by similarity of meaning; the best match is not always first, and descriptions can be incomplete or wrong, so check the code."""

TOOLS = "Bash,Read,Grep,Glob"
ARMS = ("with", "without")
VALUE_OPTIONS = {"--path", "--config", "-k", "--limit", "--kind", "--role"}
ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
SUBSTITUTION = re.compile(r"\$\(([^()`]*)\)|`([^`]*)`")


def claude_cmd(model, arm, prompt, max_turns, budget):
    cmd = [
        "claude", "-p", prompt,
        "--model", model,
        "--effort", "medium",
        "--output-format", "stream-json", "--verbose",
        "--tools", TOOLS,
        "--allowedTools", TOOLS,
        "--permission-mode", "dontAsk",
        "--strict-mcp-config",
        "--setting-sources", "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--max-turns", str(max_turns),
        "--max-budget-usd", str(budget),
    ]
    if arm == "with":
        cmd += ["--append-system-prompt", WITH_PROMPT]
    return cmd


def arm_env(arm):
    env = dict(os.environ)
    env.pop("CLAUDECODE", None)
    env["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1"
    path = [p for p in env.get("PATH", "").split(":") if p and p != env.get("ANNATAR_TRIAL_BIN")]
    if arm == "with":
        path.insert(0, env["ANNATAR_TRIAL_BIN"])
    env["PATH"] = ":".join(path)
    return env


def repo_state(repo):
    return subprocess.run(["git", "-C", repo, "status", "--porcelain"], capture_output=True,
                          text=True, check=True).stdout


def substitutions(command):
    inner = []
    while True:
        match = SUBSTITUTION.search(command)
        if not match:
            return command, inner
        inner.append(match.group(1) or match.group(2))
        command = command[:match.start()] + "_" + command[match.end():]


def shell_words(command):
    command = command.replace("\\\n", " ").replace("\n", " ; ")
    lexer = shlex.shlex(command, posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    try:
        return list(lexer)
    except ValueError:
        return re.sub(r"([;|&()$])", r" \1 ", command).split()


def annatar_calls(command):
    outer, inner = substitutions(command)
    return [call for part in [outer] + inner for call in simple_calls(part)]


def simple_calls(command):
    calls, segment = [], []
    for word in shell_words(command) + [";"]:
        if word.strip("();<>|&$"):
            segment.append(word)
            continue
        while segment and ASSIGNMENT.match(segment[0]):
            segment.pop(0)
        if segment and os.path.basename(segment[0]) == "annatar":
            calls.append(subcommand(segment[1:]))
        segment = []
    return calls


def subcommand(args):
    i = 0
    while i < len(args) and args[i].startswith("-"):
        i += 2 if args[i] in VALUE_OPTIONS else 1
    return args[i] if i < len(args) else "?"


def parse_stream(path):
    tools, annatar, result, final = {}, {"search": 0, "show": 0, "other": 0}, None, ""
    for line in Path(path).read_text().splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "assistant":
            for block in ev.get("message", {}).get("content", []):
                if block.get("type") != "tool_use":
                    continue
                name = block.get("name")
                tools[name] = tools.get(name, 0) + 1
                if name == "Bash":
                    for sub in annatar_calls(block.get("input", {}).get("command", "")):
                        annatar[sub if sub in ("search", "show") else "other"] += 1
        elif ev.get("type") == "result":
            result = ev
            final = ev.get("result") or ""
    return tools, annatar, result, final


def run(args):
    spec = tomllib.loads(Path(args.tasks).read_text())
    tasks = [t for t in spec["task"] if not args.only or t["id"] in args.only.split(",")]
    repo = os.environ["ANNATAR_TRIAL_REPO"]
    model = os.environ.get("ANNATAR_TRIAL_MODEL", "claude-sonnet-5")
    memory = Path.home() / ".claude" / "projects" / repo.replace("/", "-") / "memory"
    out = Path(args.out)
    (out / "raw").mkdir(parents=True, exist_ok=True)
    baseline = repo_state(repo)
    arms = args.arms.split(",")
    for rep in range(1, args.reps + 1):
        for i, task in enumerate(tasks):
            order = arms if (rep + i) % 2 else list(reversed(arms))
            for arm in order:
                name = f"{task['id']}-{arm}-{rep}"
                raw = out / "raw" / f"{name}.jsonl"
                if raw.exists() and '"type":"result"' in raw.read_text():
                    continue
                env = arm_env(arm)
                check_path(arm, env)
                prompt = spec["preamble"] + task["prompt"]
                start = time.time()
                with open(raw, "w") as f:
                    try:
                        returncode = subprocess.run(claude_cmd(model, arm, prompt, args.max_turns, args.budget),
                                                    cwd=repo, env=env, stdout=f, stderr=subprocess.STDOUT,
                                                    timeout=args.timeout).returncode
                    except subprocess.TimeoutExpired:
                        returncode = None
                wall = time.time() - start
                meta = {"returncode": returncode, "timeout": returncode is None, "wall_s": round(wall, 1)}
                (out / "raw" / f"{name}.meta.json").write_text(json.dumps(meta) + "\n")
                if repo_state(repo) != baseline:
                    sys.exit(f"{name}: repository changed")
                if memory.exists() and any(memory.iterdir()):
                    sys.exit(f"{name}: agent wrote memory under {memory}")
                tools, annatar, result, _ = parse_stream(raw)
                if result is None:
                    print(f"{name}: no result (exit {returncode}, timeout {meta['timeout']}) wall {wall:.0f}s",
                          flush=True)
                    continue
                used = sorted(result.get("modelUsage", {}))
                if used != [model]:
                    sys.exit(f"{name}: ran on {used}, not {model}")
                print(f"{name}: exit {returncode} wall {wall:.0f}s cost {result.get('total_cost_usd')} "
                      f"tools {tools} annatar {annatar}", flush=True)


def check_path(arm, env):
    found = shutil.which("annatar", path=env["PATH"])
    if arm == "without" and found:
        sys.exit(f"annatar is on PATH for the without arm: {found}")
    if arm == "with" and found != str(Path(env["ANNATAR_TRIAL_BIN"]) / "annatar"):
        sys.exit(f"annatar on PATH for the with arm is {found}, not the one in ANNATAR_TRIAL_BIN")


def rows(out):
    spec_rows, problems = [], []
    for raw in sorted((Path(out) / "raw").glob("*.jsonl")):
        task, arm, rep = raw.stem.split("-")
        tools, annatar, result, final = parse_stream(raw)
        meta_path = raw.with_name(f"{raw.stem}.meta.json")
        meta = json.loads(meta_path.read_text()) if meta_path.exists() else {}
        if meta.get("timeout"):
            problems.append(f"{raw.stem}: timed out")
        elif meta.get("returncode") not in (None, 0):
            problems.append(f"{raw.stem}: exit {meta['returncode']}")
        if not result:
            problems.append(f"{raw.stem}: no result event, left out")
            continue
        if result.get("subtype") != "success":
            problems.append(f"{raw.stem}: subtype {result.get('subtype')}")
        u = result["usage"]
        total = u["input_tokens"] + u["cache_read_input_tokens"] + u["cache_creation_input_tokens"] + u["output_tokens"]
        spec_rows.append({
            "run": raw.stem, "task": task, "arm": arm, "rep": int(rep),
            "subtype": result.get("subtype"), "returncode": meta.get("returncode"),
            "turns": result.get("num_turns"),
            "duration_s": round(result.get("duration_ms", 0) / 1000, 1),
            "cost_usd": round(result.get("total_cost_usd", 0), 4),
            "tokens_total": total, "input": u["input_tokens"], "cache_read": u["cache_read_input_tokens"],
            "cache_creation": u["cache_creation_input_tokens"], "output": u["output_tokens"],
            "tool_calls": sum(tools.values()),
            "bash": tools.get("Bash", 0), "read": tools.get("Read", 0), "grep": tools.get("Grep", 0),
            "glob": tools.get("Glob", 0), "other_tools": sum(v for k, v in tools.items() if k not in ("Bash", "Read", "Grep", "Glob")),
            "annatar_search": annatar["search"], "annatar_show": annatar["show"], "annatar_other": annatar["other"],
            "final_chars": len(final),
        })
    return spec_rows, problems


def summary(args):
    data, problems = rows(args.out)
    for problem in problems:
        print(f"warning: {problem}", file=sys.stderr)
    if not data:
        sys.exit("no finished runs")
    with open(Path(args.out) / "runs.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(data[0]))
        w.writeheader()
        w.writerows(data)
    keys = ["tokens_total", "cost_usd", "tool_calls", "turns", "duration_s", "annatar_search", "annatar_show"]
    groups = {}
    for r in data:
        groups.setdefault((r["task"], r["arm"]), []).append(r)
        groups.setdefault(("ALL", r["arm"]), []).append(r)
    print("task arm n " + " ".join(f"{k}(mean/min/max)" for k in keys))
    for (task, arm), rs in sorted(groups.items()):
        cells = []
        for k in keys:
            v = [r[k] for r in rs]
            cells.append(f"{statistics.mean(v):.3g}/{min(v):.3g}/{max(v):.3g}")
        print(task, arm, len(rs), " ".join(cells))


def blind(args):
    out = Path(args.out)
    runs = sorted((out / "raw").glob("*.jsonl"))
    rng = random.Random(args.seed)
    ids = rng.sample(range(100, 1000), len(runs))
    blind_dir = out / "blind"
    blind_dir.mkdir(exist_ok=True)
    mapping = {}
    for code, raw in zip(ids, runs):
        _, _, _, final = parse_stream(raw)
        text = re.sub(r"(?i)annatar", "[index]", final)
        task = raw.stem.split("-")[0]
        (blind_dir / f"{task}-{code}.md").write_text(text + "\n")
        mapping[f"{task}-{code}"] = raw.stem
    (out / "blind-mapping.json").write_text(json.dumps(mapping, indent=1, sort_keys=True) + "\n")
    print(f"{len(runs)} answers in {blind_dir}; mapping in blind-mapping.json")


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--tasks", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--reps", type=int, default=2)
    r.add_argument("--only")
    r.add_argument("--arms", default="with,without")
    r.add_argument("--max-turns", type=int, default=60)
    r.add_argument("--budget", type=float, default=3.0)
    r.add_argument("--timeout", type=int, default=1200)
    s = sub.add_parser("summary")
    s.add_argument("--out", required=True)
    b = sub.add_parser("blind")
    b.add_argument("--out", required=True)
    b.add_argument("--seed", type=int, default=54)
    args = p.parse_args()
    {"run": run, "summary": summary, "blind": blind}[args.cmd](args)


if __name__ == "__main__":
    main()
