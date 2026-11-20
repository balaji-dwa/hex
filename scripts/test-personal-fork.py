#!/usr/bin/env python3
"""Exercise the workflow's actual shell against disposable Git repositories."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest

WORKFLOW = (Path(__file__).resolve().parents[1] /
            '.github/workflows/personal-upstream.yml').read_text()


def step(name):
    block = WORKFLOW.split(f'- name: {name}\n', 1)[1]
    return textwrap.dedent(re.search(r'        run: \|\n((?:          .*\n|\n)+)', block)[1])


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=str(self.root / 'gitconfig'),
                        GIT_CONFIG_NOSYSTEM='1', GIT_AUTHOR_NAME='Test',
                        GIT_AUTHOR_EMAIL='test@example.invalid', GIT_COMMITTER_NAME='Test',
                        GIT_COMMITTER_EMAIL='test@example.invalid',
                        GITHUB_REPOSITORY='fixture/hex', GITHUB_RUN_ID='1',
                        GITHUB_RUN_ATTEMPT='1', GITHUB_SERVER_URL='https://example.invalid',
                        RUNNER_TEMP=str(self.root), GITHUB_OUTPUT=str(self.root / 'output'),
                        GITHUB_STEP_SUMMARY=str(self.root / 'summary'))
        self.cwd = self.root
        self.git('init', '--bare', 'origin')
        self.git('init', '-b', 'dev', 'upstream')
        self.cwd = self.root / 'upstream'
        self.commit('shared', 'base')
        self.git('push', str(self.root / 'origin'), 'dev')
        self.cwd = self.root
        self.git('clone', '-b', 'dev', str(self.root / 'origin'), 'fork')
        self.cwd = self.root / 'fork'
        self.commit('personal', 'keep me')
        self.git('push', 'origin', 'dev')
        self.base = self.git('rev-parse', 'HEAD')
        self.git('config', '--global', f'url.{self.root / "upstream"}.insteadOf',
                 'https://github.com/anomalyco/hex.git')
        mock = self.root / 'bin'
        mock.mkdir()
        (mock / 'gh').write_text('#!/bin/sh\ncase "$*" in\n'
                                '  "pr list "*) echo "${PENDING:-false}";;\n'
                                '  "pr create "*) touch "$RUNNER_TEMP/pr-created";;\n'
                                '  "auth setup-git") ;;\n  *) exit 1;;\nesac\n')
        (mock / 'gh').chmod(0o755)
        self.env['PATH'] = str(mock) + os.pathsep + self.env['PATH']

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.cwd,
                                       env=self.env, stderr=subprocess.DEVNULL).decode().strip()

    def commit(self, name, contents):
        (self.cwd / name).write_text(contents)
        self.git('add', name)
        self.git('commit', '-m', contents)

    def merge(self):
        subprocess.run(['bash', '-c', step('Merge upstream dev in a disposable checkout')],
                       cwd=self.cwd, env=self.env, check=True, capture_output=True)
        output = self.root / 'output'
        return dict(line.split('=', 1) for line in output.read_text().splitlines()) if output.exists() else {}

    def publish(self, result, outputs):
        artifact = self.root / 'upstream-candidate'
        artifact.mkdir()
        (self.root / 'candidate.bundle').rename(artifact / 'candidate.bundle')
        self.env.update(CANDIDATE=outputs['sha'], BRANCH=outputs['branch'],
                        CHECK_RESULT=result, MERGED=outputs['merged'])
        subprocess.run(['bash', '-c', step('Fast-forward only after checks, otherwise open a PR')],
                       cwd=self.cwd, env=self.env, check=True, capture_output=True)

    def advance_upstream(self, conflict=False):
        fork = self.cwd
        if conflict:
            self.commit('shared', 'personal edit')
            self.git('push', 'origin', 'dev')
            self.base = self.git('rev-parse', 'HEAD')
        self.cwd = self.root / 'upstream'
        self.commit('shared' if conflict else 'new', 'upstream edit')
        self.cwd = fork

    def test_success_keeps_personal_changes(self):
        self.advance_upstream()
        outputs = self.merge()
        self.publish('success', outputs)
        self.assertEqual(self.git('ls-remote', 'origin', 'dev').split()[0], outputs['sha'])
        self.assertEqual(self.git('show', 'HEAD:personal'), 'keep me')
        self.assertFalse((self.root / 'pr-created').exists())

    def test_failed_tests_open_pr_without_changing_dev(self):
        self.advance_upstream()
        self.publish('failure', self.merge())
        self.assertEqual(self.git('ls-remote', 'origin', 'dev').split()[0], self.base)
        self.assertTrue((self.root / 'pr-created').exists())

    def test_conflicts_keep_real_upstream_for_manual_merge(self):
        self.advance_upstream(conflict=True)
        outputs = self.merge()
        self.assertEqual(outputs['merged'], 'false')
        self.publish('success', outputs)
        self.assertEqual(self.git('ls-remote', 'origin', 'dev').split()[0], self.base)
        self.assertTrue((self.root / 'pr-created').exists())

    def test_concurrent_push_is_never_overwritten(self):
        self.advance_upstream()
        outputs = self.merge()
        self.git('switch', 'dev')
        self.commit('other', 'concurrent change')
        self.git('push', 'origin', 'dev')
        newer = self.git('rev-parse', 'HEAD')
        self.publish('success', outputs)
        self.assertEqual(self.git('ls-remote', 'origin', 'dev').split()[0], newer)
        self.assertTrue((self.root / 'pr-created').exists())

    def test_no_update_and_pending_pr_are_noops(self):
        self.assertEqual(self.merge(), {})
        self.advance_upstream()
        self.env['PENDING'] = 'true'
        self.assertEqual(self.merge(), {})


if __name__ == '__main__':
    unittest.main()
