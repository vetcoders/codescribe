"""The lease identity rule shared with Rust and Swift.

Every vector of ``fixtures/lease-identity-vectors.json`` must give the same
lease id in ``bus-demux.py`` as in ``core::stt::active_names::follower_lease_id``,
``core::agent::conversation_search`` and ``AgentPlaybackIdentity`` in Swift.
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
        for vector in vectors:
            with self.subTest(vector["case"]):
                self.assertEqual(
                    DEMUX.lease_identifier(vector["provider"], vector["provider_session_id"]),
                    vector["lease_id"],
                )


if __name__ == "__main__":
    unittest.main()
