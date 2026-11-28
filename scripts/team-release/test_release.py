"""Exercise release.sh against a local HTTPS origin and a mock R2 bucket."""

import http.server
import json
import os
import pathlib
import platform
import shlex
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import unittest

import manifest


class ReleaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(tmp.cleanup)
        cls.cert = pathlib.Path(tmp.name) / "cert.pem"
        key = cls.cert.with_name("key.pem")
        subprocess.run(
            [
                "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                "-keyout", str(key), "-out", str(cls.cert), "-days", "1",
                "-subj", "/CN=localhost",
                "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
            ],
            check=True,
            capture_output=True,
        )
        cls.tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        cls.tls.load_cert_chain(cls.cert, key)

    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = pathlib.Path(tmp.name)
        self.release_dir = self.root / "scripts" / "team-release"
        self.release_dir.mkdir(parents=True)
        for name in ("release.sh", "manifest.py"):
            shutil.copy(pathlib.Path(__file__).with_name(name), self.release_dir)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.stub("bin/python3", f'exec {shlex.quote(sys.executable)} "$@"')
        self.bucket = self.root / "bucket"
        self.bucket.mkdir()
        self.responses = {}
        self.requests = []
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                fixture.requests.append((self.command, self.path, dict(self.headers)))
                if self.path in fixture.responses:
                    status, headers, body = fixture.responses[self.path]
                elif self.path in ("/download", "/download/dev", "/download/stable", "/download/beta"):
                    channel = self.path.removeprefix("/download/") if self.path != "/download" else "dev"
                    obj = fixture.bucket / "hex" / "desktop" / f"{channel}.json"
                    if obj.is_file():
                        dmg_url = json.loads(obj.read_bytes())["release"]["targets"]["macos-arm64-dmg"]["url"]
                        status, headers, body = 302, {"Location": fixture.origin + dmg_url}, b""
                    else:
                        status, headers, body = 404, {}, b""
                else:
                    obj = fixture.bucket / self.path.lstrip("/")
                    status, headers, body = (200, {}, obj.read_bytes()) if obj.is_file() else (404, {}, b"")
                self.send_response(status)
                self.send_header("Content-Length", str(len(body)))
                for key, value in headers.items():
                    self.send_header(key, value)
                self.end_headers()
                if self.command == "GET":
                    self.wfile.write(body)

            do_HEAD = do_GET

            def log_message(self, *args) -> None:
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        server.socket = self.tls.wrap_socket(server.socket, server_side=True)
        self.addCleanup(server.server_close)
        thread = threading.Thread(target=server.serve_forever, args=(0.02,), daemon=True)
        thread.start()
        self.addCleanup(server.shutdown)
        self.origin = f"https://127.0.0.1:{server.server_port}"
        self.login = f"https://localhost:{server.server_port}/login"
        (self.root / ".curlrc").write_text("location\n", encoding="utf-8")

    def stub(self, path: str, script: str) -> None:
        file = self.root / path
        file.write_text("#!/bin/bash\n" + script + "\n", encoding="utf-8")
        file.chmod(0o755)

    def put_object(self, key: str, data: dict) -> None:
        file = self.bucket / key
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(json.dumps(data), encoding="utf-8")

    def run_release(self, *args: str) -> subprocess.CompletedProcess:
        env = {key: value for key, value in os.environ.items() if not key.startswith("HEX_")}
        env.update(
            PATH=f"{self.bin}:{env['PATH']}",
            HEX_TEAM_RELEASE_CONFIG=str(self.root / "missing.env"),
            HEX_TEAM_RELEASE_STATE_DIR=str(self.root / "state"),
            HEX_RELEASE_ORIGIN=self.origin,
            HEX_RELEASE_CF_ACCESS_CLIENT_ID="fixture-client",
            HEX_RELEASE_CF_ACCESS_CLIENT_SECRET="fixture-secret",
            HEX_TEAM_ID="5NZ4Q7NXJ4",
            HEX_NOTARY_PROFILE="fixture",
            HEX_R2_BUCKET="fixture",
            HEX_R2_ENDPOINT="https://unused.invalid",
            MOCK_BUCKET=str(self.bucket),
            CURL_HOME=str(self.root),
            CURL_CA_BUNDLE=str(self.cert),
            SSL_CERT_FILE=str(self.cert),
            NO_PROXY="*",
        )
        return subprocess.run(
            ["bash", str(self.release_dir / "release.sh"), *args],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def archive(self) -> pathlib.Path:
        artifact = self.root / "artifact"
        artifact.write_bytes(b"fixture")
        data = manifest.build_manifest(
            channel="dev", version="0.0.0-dev-10", sequence=7,
            zip_path=artifact, dmg_path=artifact,
        )
        archive = self.root / "archived.json"
        archive.write_text(json.dumps(data), encoding="utf-8")
        return archive

    def assert_failed(self, result: subprocess.CompletedProcess, message: str) -> None:
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_redirect_reports_access_error_without_sending_credentials_to_location(self) -> None:
        self.responses["/hex/desktop/dev.json"] = (302, {"Location": self.login}, b"")
        self.responses["/login"] = (200, {}, b"<html>Sign in</html>")
        result = self.run_release("--dry-run", "--rollback", str(self.archive()))
        self.assert_failed(result, "HTTP 302 redirect")
        self.assertIn(self.login, result.stderr)
        self.assertIn("Cloudflare Access service token", result.stderr)
        self.assertEqual([path for _, path, _ in self.requests], ["/hex/desktop/dev.json"])
        headers = self.requests[0][2]
        self.assertEqual(headers["CF-Access-Client-Id"], "fixture-client")
        self.assertEqual(headers["CF-Access-Client-Secret"], "fixture-secret")

    def test_only_missing_manifest_starts_at_sequence_one(self) -> None:
        args = ("--dry-run", "--rollback", str(self.archive()))
        for status in (500, 403):
            with self.subTest(status=status):
                self.responses["/hex/desktop/dev.json"] = (status, {}, b"")
                result = self.run_release(*args)
                self.assert_failed(result, f"HTTP {status}")
                self.assertNotIn("sequence 1", result.stdout)
        self.responses.clear()
        result = self.run_release(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout.split("\n", 1)[1])["sequence"], 1)

    def test_http_200_with_empty_or_html_body_fails(self) -> None:
        args = ("--dry-run", "--rollback", str(self.archive()))
        for body in (b"", b"<html>Sign in</html>"):
            with self.subTest(body=body):
                self.responses["/hex/desktop/dev.json"] = (200, {}, body)
                self.assert_failed(self.run_release(*args), "Expected a JSON object")

    def test_cross_channel_rollback_is_rejected(self) -> None:
        result = self.run_release("--dry-run", "--channel", "stable", "--rollback", str(self.archive()))
        self.assert_failed(result, "Cannot roll back the stable channel using a dev archive")

    def mock_publish_dependencies(self) -> None:
        (self.root / "app").mkdir()
        (self.root / "app" / "update-requirement.txt").touch()
        self.stub("scripts/macos-signing.sh", "hex_codesign_identity() { echo fixture; }\nhex_build_number() { echo 1; }")
        self.stub(
            "scripts/build-app.sh",
            'printf "%s" "$HEX_RELEASE_DOWNLOAD_URL" > download-url\n'
            'mkdir fixture.app\n'
            'echo "$PWD/fixture.app"',
        )
        for path in ("scripts/validate-app.sh", "scripts/test-app-identity.sh", "scripts/check", "bin/codesign", "bin/spctl"):
            self.stub(path, "exit 0")
        self.stub("scripts/notarize-app.sh", 'echo "fixture DMG" > "$2"')
        self.stub(
            "bin/git",
            'case "$3" in\n'
            '  status) ;;\n'
            '  rev-parse) echo fixture-commit ;;\n'
            '  log) echo "Fixture release" ;;\n'
            '  *) exit 1 ;;\n'
            'esac',
        )
        self.stub(
            "bin/aws",
            '[[ "$5 $6" == "s3 cp" ]] || exit 1\n'
            'key="${8#s3://fixture/}"\n'
            'mkdir -p "$MOCK_BUCKET/$(dirname "$key")"\n'
            'cp "$7" "$MOCK_BUCKET/$key"',
        )

    @unittest.skipUnless(platform.system() == "Darwin" and platform.machine() == "arm64", "requires macOS arm64 packaging tools")
    def test_stable_publish_verifies_channel_download_and_preserves_notes(self) -> None:
        self.mock_publish_dependencies()
        previous = {"version": "3.5.1", "commit": "older-commit", "items": []}
        archived = json.loads(self.archive().read_bytes())
        archived.update(channel="stable", sequence=41)
        self.put_object("hex/desktop/stable.json", archived)
        self.put_object("hex/files/dev-notes.json", {"builds": [previous]})
        result = self.run_release("--channel", "stable", "--version", "3.5.2")
        self.assertEqual(result.returncode, 0, result.stderr)
        download_url = self.origin + "/download/stable"
        self.assertIn(f"Download: {download_url}", result.stdout)
        self.assertEqual((self.root / "download-url").read_text(), download_url)
        download_checks = [(method, path) for method, path, _ in self.requests if path.startswith("/download")]
        self.assertEqual(download_checks, [("HEAD", "/download/stable")])
        live = json.loads((self.bucket / "hex" / "desktop" / "stable.json").read_bytes())
        self.assertEqual(live["sequence"], 42)
        notes = json.loads((self.bucket / "hex" / "files" / "dev-notes.json").read_bytes())
        self.assertEqual(notes["builds"][1], previous)

    @unittest.skipUnless(platform.system() == "Darwin" and platform.machine() == "arm64", "requires macOS arm64 packaging tools")
    def test_notes_fetch_error_prevents_uploads(self) -> None:
        self.mock_publish_dependencies()
        self.responses["/hex/files/dev-notes.json"] = (500, {}, b"")
        result = self.run_release("--version", "3.5.2")
        self.assert_failed(result, "/hex/files/dev-notes.json (HTTP 500)")
        self.assertEqual(list(self.bucket.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
