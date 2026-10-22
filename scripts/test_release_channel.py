import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

spec = importlib.util.spec_from_file_location("release_channel", Path(__file__).with_name("release-channel.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
NS = release.NS


def item(build, version, channel=None):
    channel = f"<sparkle:channel>{channel}</sparkle:channel>" if channel else ""
    return f'''<item><title>{version}</title><sparkle:version>{build}</sparkle:version>
      <sparkle:shortVersionString>{version}</sparkle:shortVersionString>{channel}
      <enclosure url="https://example.org/Scrawl-{version}-arm64.zip" length="123" sparkle:edSignature="signed-bytes"/>
      <sparkle:deltas><enclosure url="delta" sparkle:edSignature="signed-delta"/></sparkle:deltas></item>'''


def feed(*items):
    return f'<rss xmlns:sparkle="{NS}"><channel><title>Scrawl</title>{"".join(items)}</channel></rss>'


class ReleaseTests(unittest.TestCase):
    def test_version_ordering(self):
        versions = ["0.3.14", "0.3.14-beta.10", "0.3.13", "0.3.14-beta.2"]
        self.assertEqual(sorted(versions, key=release.parse_version),
                         ["0.3.13", "0.3.14-beta.2", "0.3.14-beta.10", "0.3.14"])

    def test_invalid_versions(self):
        for version in ["0.3", "0.3.14-beta.0", "0.3.14-beta.x", "0.3.14;exit", "0.3.14-rc.1"]:
            with self.assertRaises(ValueError):
                release.parse_version(version)

    def test_next_beta_after_stable(self):
        self.assertEqual(release.next_beta("0.3.13", ["v0.3.13"]), "0.3.14-beta.1")
        self.assertEqual(release.next_beta("0.3.14", ["v0.3.13", "v0.3.14-beta.9"]), "0.3.14-beta.10")
        self.assertEqual(release.next_beta("0.3.13", ["v0.4.0-beta.2"]), "0.4.0-beta.3")
        self.assertEqual(release.next_beta("0.3.13", ["v0.3.14", "v0.3.14-beta.9"]), "0.3.15-beta.1")

    def test_builds_include_failed_reservations_and_both_channels(self):
        xml = feed(item(16, "0.3.13"), item(18, "0.3.14-beta.2", "beta"))
        self.assertEqual(release.next_build(xml, [{"build": 19}]), 20)

    def test_promote_preserves_signed_enclosure_and_beta(self):
        xml = feed(item(16, "0.3.13"), item(18, "0.3.14-beta.2", "beta"))
        promoted = release.promoted_feed(xml, 18, "0.3.14")
        items = release.feed_items(promoted)
        self.assertEqual(len(items), 3)
        same_build = [i for i in items if release.build_number(i) == 18]
        self.assertEqual({i.findtext(f"{{{NS}}}channel") for i in same_build}, {None, "beta"})
        self.assertEqual(ET.tostring(same_build[0].find("enclosure")), ET.tostring(same_build[1].find("enclosure")))
        self.assertEqual(ET.tostring(same_build[0].find(f"{{{NS}}}deltas")), ET.tostring(same_build[1].find(f"{{{NS}}}deltas")))
        self.assertEqual(release.promoted_feed(promoted, 18, "0.3.14"), promoted)

    def test_promote_refuses_downgrade(self):
        xml = feed(item(20, "0.3.15"), item(18, "0.3.14-beta.2", "beta"))
        with self.assertRaises(ValueError):
            release.promoted_feed(xml, 18, "0.3.14")

    def test_immutable_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            a, b = Path(directory) / "a", Path(directory) / "b"
            a.write_bytes(b"signed original")
            release.immutable_copy(a, b)
            release.immutable_copy(a, b)
            a.write_bytes(b"rebuilt")
            with self.assertRaises(ValueError):
                release.immutable_copy(a, b)
            self.assertEqual(b.read_bytes(), b"signed original")

    def test_job_only_inserts_requested_build_on_beta_channel(self):
        import json
        with tempfile.TemporaryDirectory() as directory, patch.object(release, "STORE", Path(directory)):
            store = Path(directory)
            (store / "updates").mkdir()
            (store / "updates/appcast.xml").write_text(feed(item(16, "0.3.13"), item(18, "0.3.14-beta.2", "beta")))
            request = {"manifest": {"build": 18, "zip": "Scrawl-0.3.14-beta.2-arm64.zip"},
                       "channel": "beta", "result": str(store / "result.json")}
            (store / "appcast-request.json").write_text(json.dumps(request))
            with patch.object(release, "run", side_effect=["--channel", release.PUBLIC_KEY, "", ""]) as run:
                release.appcast_job()
                args = run.call_args_list[2].args
                self.assertEqual(args[1:7], ("--versions", 18, "--channel", "beta", "--maximum-versions", "0"))
            self.assertEqual(json.loads((store / "result.json").read_text()), {})

    def test_completed_signing_is_reused_after_an_interrupted_upload(self):
        import json
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            (folder / "build-result.json").write_text(json.dumps({}))
            with patch.object(release.subprocess, "run") as process, patch.object(release, "run") as run:
                process.return_value.returncode = 1  # No leftover launchd job.
                release.gui_build({"build": 18}, folder)
                run.assert_not_called()

    def test_wrong_signing_key_stops_before_mutating_feed(self):
        import json
        with tempfile.TemporaryDirectory() as directory, patch.object(release, "STORE", Path(directory)):
            store = Path(directory)
            request = {"manifest": {"build": 18}, "channel": "beta", "result": str(store / "result.json")}
            (store / "appcast-request.json").write_text(json.dumps(request))
            with patch.object(release, "run", side_effect=["--channel", "wrong-key"]):
                with self.assertRaisesRegex(RuntimeError, "SUPublicEDKey"):
                    release.appcast_job()
            self.assertIn("SUPublicEDKey", json.loads((store / "result.json").read_text())["error"])
            self.assertFalse((store / "updates").exists())

    def test_interrupted_appcast_submission_resumes_the_same_request(self):
        import json
        with tempfile.TemporaryDirectory() as directory, patch.object(release, "STORE", Path(directory)):
            store = Path(directory)
            manifest = {"build": 18}
            result = store / "result.json"
            request = {"manifest": manifest, "channel": "beta", "result": str(result)}
            (store / "appcast-request.json").write_text(json.dumps(request))
            with patch.object(release, "run") as run, patch.object(release, "wait_file", side_effect=lambda p: p.write_text("{}")):
                release.appcast(manifest, "beta")
                self.assertEqual(run.call_count, 1)
                self.assertEqual(run.call_args.args[:2], ("launchctl", "kickstart"))
            self.assertEqual(json.loads((store / "appcast-request.json").read_text()), request)


if __name__ == "__main__":
    unittest.main()
