import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("recovery", Path(__file__).with_name("reconcile-device-history.py"))
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)


def turn(identity, host, source, timestamp):
    return dict(event_id=identity, host_id=host, agent="claude-code", session_id="session",
                source=source, event_type="turn", ts=timestamp)


class RecoveryTests(unittest.TestCase):
    def test_only_mutually_unique_ordered_prompts_are_matched(self):
        current = [turn("a", "new", "log", 50), turn("b", "new", "log", 2000),
                   turn("slash-command", "new", "log", 7000)]
        previous = [turn("x", "old", "otel", 55), turn("y", "old", "otel", 1995)]
        planner = recovery.Recovery(current, previous)
        planner.run(True)
        self.assertEqual({k: a["remote_id"] for k, a in planner.aliases.items()}, {"a": "x", "b": "y"})
        with self.assertRaises(ValueError):
            recovery.Recovery(current, previous).run(False)
        with self.assertRaises(ValueError):
            recovery.Recovery(current + [turn("other", "new", "log", 70)], previous).run(True)

    def test_equal_observations_preserve_multiplicity(self):
        planner = recovery.Recovery([turn("a", "new", "log", 80), turn("b", "new", "log", 80)],
                                    [turn("x", "old", "log", 80)])
        planner.run(False)
        self.assertEqual(len(planner.aliases), 1)


if __name__ == "__main__":
    unittest.main()
