# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Regression tests for the measurement guard's process-event classifier."""
import tempfile
import os
import shutil
import sys
from pathlib import Path
import unittest

from guard import trace_violations
from qualify import equivalent, snapshot, run_case, replay


class ProcessEvents(unittest.TestCase):
    def test_failed_missing_helper_is_still_a_fallback_attempt(self):
        events = '12 execve("/candidate", ["/candidate"], []) = 0\n'
        events += '12 execve("/missing/git", ["git"], []) = -1 ENOENT\n'
        self.assertEqual(len(trace_violations(events)), 1)

    def test_threads_are_allowed_but_child_processes_are_not(self):
        events = '12 execve("/candidate", [], []) = 0\n'
        self.assertFalse(trace_violations(events + '12 clone(flags=CLONE_VM|CLONE_THREAD) = 14\n'))
        self.assertTrue(trace_violations(events + '12 clone(flags=CLONE_VM|SIGCHLD) = 14\n'))
        self.assertTrue(trace_violations(events + '12 vfork() = -1 EPERM\n'))

    def test_missing_trace_cannot_qualify(self):
        self.assertTrue(trace_violations(''))

    def test_snapshot_detects_content_and_empty_directories(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root)
            before = snapshot(path)
            (path / 'empty').mkdir()
            self.assertNotEqual(before, snapshot(path))
            (path / 'file').write_bytes(b'abc')
            before = snapshot(path)
            (path / 'file').write_bytes(b'abd')
            self.assertNotEqual(before, snapshot(path))

    def test_comparison_preserves_bytes_and_stream_routing(self):
        left = dict(returncode=0, stdout='0d0a', stderr='', state=[])
        self.assertFalse(equivalent(left, dict(left, stdout='0a')))
        self.assertFalse(equivalent(left, dict(left, stdout='', stderr='0d0a')))
        self.assertFalse(equivalent(left, dict(left, returncode=-15)))

    def test_relocated_replay_preserves_symlinks_and_home_prestate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def setup(work, environment):
                (work / 'target').write_bytes(b'fixture')
                try:
                    os.symlink('target', work / 'link')
                except OSError as error:
                    self.skipTest(f'fixture filesystem cannot create a symlink: {error}')
                (Path(environment['HOME']) / 'marker').write_bytes(b'home fixture')

            program = ('import os; from pathlib import Path; '
                       'assert Path("link").is_symlink(); '
                       'assert Path("link").read_bytes() == b"fixture"; '
                       'assert (Path(os.environ["HOME"])/"marker").read_bytes() == b"home fixture"')
            result = run_case(root / 'case', Path(sys.executable), ['-c', program], setup=setup)
            self.assertEqual(result['returncode'], 0)
            shutil.copytree(root / 'case', root / 'relocated', symlinks=True)
            self.assertEqual(replay(root / 'relocated'), 0)


if __name__ == '__main__':
    unittest.main()
