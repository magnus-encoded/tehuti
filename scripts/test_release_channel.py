import os
import pathlib
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]


class Promotion(unittest.TestCase):
    def test_missing_smoke_asset_prevents_promotion(self):
        with tempfile.TemporaryDirectory() as directory:
            fake = pathlib.Path(directory) / 'gh'
            fake.write_text('#!/bin/sh\ncase "$*" in\n*--json*) echo start-screen.png;;\n*) exit 99;;\nesac\n')
            fake.chmod(0o755)
            result = subprocess.run(
                ['bash', str(ROOT / 'scripts/promote-release'), 'v1.0.0'],
                env={**os.environ, 'PATH': directory + ':' + os.environ['PATH']},
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('bar.png', result.stderr)

    def test_complete_smoke_assets_allow_promotion(self):
        with tempfile.TemporaryDirectory() as directory:
            fake = pathlib.Path(directory) / 'gh'
            log = pathlib.Path(directory) / 'edit'
            fake.write_text(f'''#!/bin/sh
case "$*" in
*--json*) printf '%s\\n' start-screen.png bar.png launcher.png settings.png keyboard.png;;
*) printf '%s' "$*" > '{log}';;
esac
''')
            fake.chmod(0o755)
            subprocess.run(['bash', str(ROOT / 'scripts/promote-release'), 'v1.0.0'],
                           env={**os.environ, 'PATH': directory + ':' + os.environ['PATH']}, check=True)
            self.assertEqual(log.read_text(), 'release edit v1.0.0 --prerelease=false --latest')
