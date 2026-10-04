import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import run

FIXTURES = Path(__file__).parent / "fixtures"


class AnnatarCallsTest(unittest.TestCase):
    def test_plain_and_chained(self):
        self.assertEqual(run.annatar_calls('annatar search "a b" && annatar show \'x.Y\''), ["search", "show"])
        self.assertEqual(run.annatar_calls("annatar search a; annatar search b | head -20"), ["search", "search"])
        self.assertEqual(run.annatar_calls("annatar search a\nannatar show b"), ["search", "show"])

    def test_path_and_no_call(self):
        self.assertEqual(run.annatar_calls("/opt/bin/annatar show x"), ["show"])
        self.assertEqual(run.annatar_calls("grep -rn annatar src"), [])
        self.assertEqual(run.annatar_calls('echo "annatar search x"'), [])

    def test_env_assignments_and_subshells(self):
        self.assertEqual(run.annatar_calls("RUST_LOG=off A=1 annatar search q"), ["search"])
        self.assertEqual(run.annatar_calls('echo "$(annatar search q)"'), ["search"])
        self.assertEqual(run.annatar_calls("x=$(annatar show a)"), ["show"])
        self.assertEqual(run.annatar_calls("(cd /r && annatar search q)"), ["search"])
        self.assertEqual(run.annatar_calls("x=`annatar show a`"), ["show"])

    def test_global_options_before_subcommand(self):
        self.assertEqual(run.annatar_calls("annatar -k 5 search q"), ["search"])
        self.assertEqual(run.annatar_calls("annatar --config a.toml -v search q"), ["search"])
        self.assertEqual(run.annatar_calls("annatar --path=src show x"), ["show"])
        self.assertEqual(run.annatar_calls("annatar --help"), ["?"])

    def test_trace(self):
        self.assertEqual(run.annatar_calls("annatar trace 'a.B#c()' --depth 8 | head"), ["trace"])
        self.assertEqual(run.annatar_calls("annatar --config x trace a.B --depth 8 && annatar show a.B"), ["trace", "show"])

    def test_show_history(self):
        self.assertEqual(run.annatar_calls("annatar show --history 'a.B#c()' | head"), ["show --history"])
        self.assertEqual(run.annatar_calls("annatar -k 5 show a.B --history; annatar show a.B -k 5"),
                         ["show --history", "show"])
        self.assertEqual(run.annatar_calls('annatar search "show --history"'), ["search"])
        self.assertEqual(run.annatar_calls("annatar show -k 5 --history a.B"), ["show --history"])
        self.assertEqual(run.annatar_calls("x=$(annatar show --history a.B)"), ["show --history"])
        self.assertEqual(run.annatar_calls("annatar show a.B | grep -- --history"), ["show"])

    def test_loops_and_prefix_commands(self):
        self.assertEqual(run.annatar_calls("for s in a.B a.C; do annatar show \"$s\"; done"), ["show"])
        self.assertEqual(run.annatar_calls("if true; then annatar search q; else annatar show a.B; fi"),
                         ["search", "show"])
        self.assertEqual(run.annatar_calls("time annatar search q"), ["search"])
        self.assertEqual(run.annatar_calls("env RUST_LOG=off annatar trace a.B"), ["trace"])
        self.assertEqual(run.annatar_calls("echo a.B | xargs -I{} annatar show {} --history"), ["show --history"])
        self.assertEqual(run.annatar_calls("echo a.B | xargs -n 1 annatar show"), ["show"])
        self.assertEqual(run.annatar_calls("echo do annatar search q"), [])

    def test_query_with_separators_stays_one_call(self):
        self.assertEqual(run.annatar_calls('annatar search "a; annatar show b"'), ["search"])

    def test_unbalanced_quotes_fall_back(self):
        self.assertEqual(run.annatar_calls("annatar search 'q && annatar show x"), ["search", "show"])


class StreamTest(unittest.TestCase):
    def test_parse_stream(self):
        tools, annatar, result, final = run.parse_stream(FIXTURES / "raw" / "T1-with-1.jsonl")
        self.assertEqual(tools, {"Bash": 2, "Read": 1})
        self.assertEqual(annatar, {"search": 2, "show": 1, "show_history": 0, "trace": 0, "other": 0})
        self.assertEqual(result["subtype"], "success")
        self.assertEqual(final, "The cap is in TokenStore.")

    def test_parse_stream_counts_show_history_among_shows(self):
        command = "annatar show --history a.B && annatar show a.C && annatar trace a.B"
        event = {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "Bash", "input": {"command": command}}]}}
        with tempfile.TemporaryDirectory() as d:
            stream = Path(d) / "T2-with-1.jsonl"
            stream.write_text(json.dumps(event) + "\n")
            _, annatar, _, _ = run.parse_stream(stream)
        self.assertEqual(annatar, {"search": 0, "show": 2, "show_history": 1, "trace": 1, "other": 0})

    def test_parse_stream_without_result(self):
        tools, annatar, result, final = run.parse_stream(FIXTURES / "raw" / "T1-with-2.jsonl")
        self.assertEqual(tools, {"Bash": 1})
        self.assertIsNone(result)
        self.assertEqual(final, "")

    def test_rows_and_problems(self):
        data, problems = run.rows(FIXTURES)
        self.assertEqual([r["run"] for r in data], ["T1-with-1", "T1-without-1"])
        first = data[0]
        self.assertEqual((first["task"], first["arm"], first["rep"]), ("T1", "with", 1))
        self.assertEqual(first["tokens_total"], 6 + 1000 + 200 + 50)
        self.assertEqual(first["tool_calls"], 3)
        self.assertEqual((first["annatar_search"], first["annatar_show"], first["annatar_show_history"],
                          first["annatar_trace"]), (2, 1, 0, 0))
        self.assertIsNone(first["returncode"])
        self.assertEqual(data[1]["returncode"], 1)
        self.assertEqual((data[1]["grep"], data[1]["glob"]), (1, 1))
        self.assertEqual(problems, [
            "T1-with-2: timed out",
            "T1-with-2: no result event, left out",
            "T1-without-1: exit 1",
            "T1-without-1: subtype error_max_turns",
        ])


class ClaudeCmdTest(unittest.TestCase):
    def appended(self, cmd):
        return cmd[cmd.index("--append-system-prompt") + 1] if "--append-system-prompt" in cmd else None

    def test_with_prompt_variants(self):
        files = run.claude_cmd("m", "with", "q", 60, 3.0)
        self.assertEqual(self.appended(files), run.WITH_PROMPTS["files"])
        self.assertIn("files", self.appended(files).splitlines()[1])
        self.assertIn("`--symbols`", self.appended(files))
        symbols = run.claude_cmd("m", "with", "q", 60, 3.0, "symbols")
        self.assertEqual(self.appended(symbols), run.WITH_PROMPTS["symbols"])
        self.assertIn("(default 10)", self.appended(symbols))
        self.assertEqual(files[:files.index("--append-system-prompt")],
                         symbols[:symbols.index("--append-system-prompt")])

    def test_brief_prompt_changes_only_the_show_bullet_of_usages(self):
        brief = run.claude_cmd("m", "with", "q", 60, 3.0, "brief")
        self.assertEqual(self.appended(brief), run.WITH_PROMPTS["brief"])
        lines, usages = run.WITH_PROMPTS["brief"].splitlines(), run.WITH_PROMPTS["usages"].splitlines()
        self.assertEqual([i for i, (a, b) in enumerate(zip(lines, usages)) if a != b], [2])
        self.assertIn("`--history` adds", lines[2])

    def test_usages_prompt_adds_one_bullet_to_files(self):
        usages = run.claude_cmd("m", "with", "q", 60, 3.0, "usages")
        self.assertEqual(self.appended(usages), run.WITH_PROMPTS["usages"])
        lines = run.WITH_PROMPTS["usages"].splitlines()
        self.assertEqual(lines[:3] + lines[4:], run.WITH_PROMPTS["files"].splitlines())
        self.assertTrue(lines[3].startswith("- `annatar show` also lists `used by`"))
        self.assertIn("`annatar trace '<fqn>'`", lines[3])
        self.assertIn("(`--depth N`, default 6)", lines[3])

    def test_without_arm_gets_no_prompt(self):
        for variant in run.WITH_PROMPTS:
            self.assertIsNone(self.appended(run.claude_cmd("m", "without", "q", 60, 3.0, variant)))

    def test_variants_differ_only_in_the_tool_lines(self):
        files, symbols = (run.WITH_PROMPTS[v].splitlines() for v in ("files", "symbols"))
        self.assertEqual((files[0], files[3]), (symbols[0], symbols[3]))


class PathTest(unittest.TestCase):
    def test_annatar_only_on_the_with_path(self):
        old = dict(os.environ)
        with tempfile.TemporaryDirectory() as bin_dir, tempfile.TemporaryDirectory() as other:
            for d in (bin_dir, other):
                tool = Path(d) / "annatar"
                tool.write_text("#!/bin/sh\n")
                tool.chmod(0o755)
            try:
                os.environ["ANNATAR_TRIAL_BIN"] = bin_dir
                os.environ["PATH"] = f"{bin_dir}:/usr/bin"
                without, with_ = run.arm_env("without"), run.arm_env("with")
                self.assertNotIn(bin_dir, without["PATH"].split(":"))
                self.assertEqual(with_["PATH"].split(":")[0], bin_dir)
                run.check_path("without", without)
                run.check_path("with", with_)
                without["PATH"] = f"{other}:/usr/bin"
                with self.assertRaises(SystemExit):
                    run.check_path("without", without)
                with_["PATH"] = f"{other}:{bin_dir}"
                with self.assertRaises(SystemExit):
                    run.check_path("with", with_)
            finally:
                os.environ.clear()
                os.environ.update(old)


if __name__ == "__main__":
    unittest.main()
