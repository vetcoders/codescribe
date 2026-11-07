"""The lease identity rule shared with Rust and Swift.

Every vector of ``fixtures/lease-identity-vectors.json`` must give the same
lease id in ``bus-demux.py`` as in ``core::stt::active_names::follower_lease_id``
and ``AgentPlaybackIdentity`` in Swift, and every refused provider must be
refused by all three.
"""

import importlib.util
import json
from pathlib import Path
import unittest

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_lease_identity", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DEMUX)
VECTORS = Path(__file__).resolve().parent / "fixtures" / "lease-identity-vectors.json"


class LeaseIdentityVectorsTest(unittest.TestCase):
    def test_every_vector_gives_its_lease_id(self) -> None:
        vectors = json.loads(VECTORS.read_text(encoding="utf-8"))["vectors"]
        self.assertTrue(vectors)
        self.assertTrue(any(vector["lease_id"] is None for vector in vectors))
        for vector in vectors:
            with self.subTest(vector["case"]):
                if vector["lease_id"] is None:
                    with self.assertRaisesRegex(ValueError, "ASCII client id"):
                        DEMUX.provider_token(vector["provider"])
                    with self.assertRaises(ValueError):
                        DEMUX.lease_identifier(vector["provider"], vector["provider_session_id"])
                    continue
                self.assertEqual(DEMUX.provider_token(vector["provider"]),
                                 vector["normalized_provider"])
                self.assertEqual(
                    DEMUX.lease_identifier(vector["provider"], vector["provider_session_id"]),
                    vector["lease_id"],
                )


if __name__ == "__main__":
    unittest.main()
