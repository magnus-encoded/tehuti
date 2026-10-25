"""Behaviour of the context generator's command-line boundary."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent
FIXTURES = ROOT / 'fixtures/generate-context'
GENERATOR = ROOT / 'tehuti-generate-context'


class GenerateContextTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.work = Path(self.directory.name)
        self.env = {**os.environ, 'TEHUTI_FEATURES': '', 'TEHUTI_TICKER': 'false',
                    'TEHUTI_BIN': str(self.work / 'absent')}

    def run_generator(self, *options, success=True, parser='supertux2'):
        config = 'supertux2.config' if parser == 'supertux2' else 'mgba.ini'
        result = subprocess.run([str(GENERATOR), '--application', str(FIXTURES / 'sample.desktop'),
                                 '--config-parser', str(ROOT / 'context-parsers' / parser),
                                 '--config', str(FIXTURES / config), '--out', str(self.work),
                                 *options], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0 if success else 1, result.stderr)
        return result

    def checker(self, body):
        path = self.work / 'checker'
        path.write_text('#!/bin/sh\nif [ "$1" = --help ]; then echo --check-config; exit 0; fi\n' + body)
        path.chmod(0o755)
        self.env['TEHUTI_BIN'] = str(path)

    def test_parser_json_matches_fixtures(self):
        for parser, config in [('supertux2', 'supertux2.config'), ('mgba', 'mgba.ini')]:
            with self.subTest(parser=parser):
                result = subprocess.run([str(ROOT / 'context-parsers' / parser), str(FIXTURES / config)],
                                        capture_output=True, text=True, check=True)
                self.assertEqual(json.loads(result.stdout), json.loads((FIXTURES / (parser + '.json')).read_text()))

    def test_golden_contexts_and_idempotence(self):
        for parser in ('supertux2', 'mgba'):
            for features in ('off', 'on'):
                with self.subTest(parser=parser, features=features):
                    path = self.work / 'context.toml'
                    path.unlink(missing_ok=True)
                    options = ['--features', 'ticker,label,hold,fuzzy', '--ticker-enabled=true'] if features == 'on' else []
                    self.run_generator('--write', *options, parser=parser)
                    self.assertEqual(path.read_text(), (FIXTURES / f'{parser}.{features}.toml').read_text())
                    self.assertEqual(self.run_generator('--write', *options, parser=parser).stdout, '')

    def test_diff_does_not_write(self):
        self.assertIn('+app_id = "org.example.Game"', self.run_generator().stdout)
        self.assertFalse((self.work / 'context.toml').exists())

    def test_preserves_hand_edits_and_backup(self):
        self.run_generator('--write')
        path = self.work / 'context.toml'
        original = '# hand edit\r\n' + path.read_text() + '\n[dim]\nenabled = false\n'
        path.write_bytes(original.encode())
        self.run_generator('--write', '--kind', 'tool', '--features', 'hold')
        # Changing the kind does not change this generated block when hold is off for tools.
        self.assertEqual(path.read_bytes(), original.encode())
        self.run_generator('--write', '--features', 'hold')
        self.assertTrue(path.read_bytes().startswith(b'# hand edit\r\n'))
        self.assertTrue(path.read_text().endswith('[dim]\nenabled = false\n'))
        self.assertEqual((self.work / 'context.toml.bak-gen').read_bytes(), original.encode())

    def test_validation_errors_restore_existing_file(self):
        self.run_generator('--write')
        path = self.work / 'context.toml'
        original = path.read_bytes()
        self.checker('test "$1" = --check-config || exit 2\nexit 1\n')
        self.run_generator('--write', '--features', 'label', success=False)
        self.assertEqual(path.read_bytes(), original)
        self.assertEqual((self.work / 'context.toml.bak-gen').read_bytes(), original)

    def test_validation_errors_remove_new_file(self):
        self.checker('exit 1\n')
        self.run_generator('--write', success=False)
        self.assertFalse((self.work / 'context.toml').exists())

    def test_supported_checker_receives_output_directory(self):
        self.checker('test "$1" = --check-config && test -f "$2/context.toml"\n')
        self.run_generator('--write')

    def test_unsupported_checker_is_not_called_with_check_option(self):
        path = self.work / 'checker'
        path.write_text('#!/bin/sh\n[ "$1" = --help ] || exit 1\necho usage\n')
        path.chmod(0o755)
        self.env['TEHUTI_BIN'] = str(path)
        self.run_generator('--write')

    def test_environment_and_explicit_feature_overrides(self):
        self.env.update(TEHUTI_FEATURES='ticker,label,hold,fuzzy', TEHUTI_TICKER='true')
        self.run_generator('--write', '--features', '', '--ticker-enabled=false')
        self.assertEqual((self.work / 'context.toml').read_text(), (FIXTURES / 'supertux2.off.toml').read_text())
        self.run_generator('--write')
        self.assertEqual((self.work / 'context.toml').read_text(), (FIXTURES / 'supertux2.on.toml').read_text())

    def test_tool_kind_uses_taps_and_disables_ticker_by_default(self):
        self.env.pop('TEHUTI_TICKER')
        self.run_generator('--write', '--kind', 'tool', '--features', 'ticker,hold')
        text = (self.work / 'context.toml').read_text()
        self.assertIn('enabled = false', text)
        self.assertNotIn('hold =', text)

    def test_unmarked_context_is_never_overwritten(self):
        path = self.work / 'context.toml'
        path.write_text('app_id = "custom"\n[buttons]\n')
        self.run_generator('--write', success=False)
        self.assertEqual(path.read_text(), 'app_id = "custom"\n[buttons]\n')

    def test_missing_application_id_requires_unambiguous_probe(self):
        desktop = self.work / 'sample.desktop'
        desktop.write_text('[Desktop Entry]\nName=Sample\n')
        self.run_generator('--application', str(desktop), success=False)
        log = self.work / 'session.log'
        log.write_text('app_id="org.example.Probed"\n')
        result = self.run_generator('--application', str(desktop), '--probe', str(log))
        self.assertIn('org.example.Probed', result.stdout)
        log.write_text('app_id="one"\napp_id="two"\n')
        self.run_generator('--application', str(desktop), '--probe', str(log), success=False)

    def test_parser_failure_and_invalid_bindings_do_not_write(self):
        parser = self.work / 'parser'
        for output in ('not json', '{"actions":{"jump":"ctrl+unknown"}}', '{"actions":[]}'):
            parser.write_text('#!/usr/bin/env python3\nprint(' + repr(output) + ')\n')
            parser.chmod(0o755)
            self.run_generator('--write', '--config-parser', str(parser), success=False)
            self.assertFalse((self.work / 'context.toml').exists())

    def test_commands_use_modifiers_and_gated_icons(self):
        parser = self.work / 'parser'
        parser.write_text('#!/usr/bin/env python3\nprint(\'{"commands":{"save":"shift+f1"}}\')\n')
        parser.chmod(0o755)
        off = self.run_generator('--config-parser', str(parser)).stdout
        self.assertIn('"save" = { key = "f1", mods = ["shift"] }', off)
        self.assertNotIn('icon =', off)
        on = self.run_generator('--config-parser', str(parser), '--ticker-enabled').stdout
        self.assertIn('icon = "💾"', on)

    def test_rejects_conflicting_roles_and_normalized_speech_collisions(self):
        parser = self.work / 'parser'
        for roles in ({'actions': {'jump': 'enter', 'confirm': 'space'}},
                      {'commands': {'Eksempelspill!': 'enter'}}):
            parser.write_text('#!/usr/bin/env python3\nprint(' + repr(json.dumps(roles)) + ')\n')
            parser.chmod(0o755)
            self.run_generator('--write', '--config-parser', str(parser), success=False)
            self.assertFalse((self.work / 'context.toml').exists())

    def test_rejects_unknown_features_and_malformed_markers(self):
        self.run_generator('--features', 'unknown', success=False)
        path = self.work / 'context.toml'
        for text in ('# generated end\n# generated begin\n',
                     '# generated begin\n# generated begin\n# generated end\n'):
            path.write_text(text)
            self.run_generator('--write', success=False)
            self.assertEqual(path.read_text(), text)

    def test_parser_reads_sdl_and_qt_modifier_bindings(self):
        config = self.work / 'config.ini'
        config.write_text('[gba.input.SDLK]\nkeyA=120\nkeyUp=1073741906\n')
        result = subprocess.run([str(ROOT / 'context-parsers/mgba'), str(config)],
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(result.stdout),
                         {'actions': {'confirm': 'x', 'up': 'up'}, 'commands': {}})
        config.write_text('[gba.input.QT_K]\nkeyA=33554520\n')
        result = subprocess.run([str(ROOT / 'context-parsers/mgba'), str(config)],
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(result.stdout),
                         {'actions': {'confirm': 'shift+x'}, 'commands': {}})

    def test_parser_errors_exit_without_changing_context(self):
        config = self.work / 'broken'
        for parser, text in (('mgba', '[general]\n'),
                             ('supertux2', '(supertux-config (control')):
            config.write_text(text)
            self.run_generator('--write', '--config', str(config), parser=parser, success=False)
            self.assertFalse((self.work / 'context.toml').exists())


if __name__ == '__main__':
    unittest.main()
