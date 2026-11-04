# SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
# SPDX-License-Identifier: GPL-2.0-only

"""Regression tests for the measurement guard's process-event classifier."""
import tempfile
from pathlib import Path
import unittest

from guard import trace_violations
from qualify import equivalent, snapshot


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


if __name__ == '__main__':
    unittest.main()
