import importlib.util
from pathlib import Path
import sys
import unittest


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).parents[1] / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


DEMUX = load("address_authority_demux", "bus-demux.py")
MCP = load("address_authority_mcp", "bus-mcp.py")


class BusAddressAuthorityTests(unittest.TestCase):
    def test_peer_broadcast_copy_is_addressed_to_channel_zero_not_its_recipient(self):
        # send_peer_command gives each channel-0 copy its recipient's name in
        # peer_to for routing; a direct message carries the recipient's digit.
        payload = {"channel": "0", "peer_to": "lena", "audience": "lena"}
        self.assertEqual(DEMUX.delivery_address_label(payload), "0 (all audience)")
        self.assertTrue(DEMUX.is_broadcast_delivery(payload))

    def test_direct_peer_copy_names_its_recipient_channel(self):
        payload = {"channel": "2", "peer_to": "lena", "audience": "lena"}
        self.assertEqual(DEMUX.delivery_address_label(payload), "2 (lena)")
        self.assertFalse(DEMUX.is_broadcast_delivery(payload))

    def test_founder_broadcast_keeps_source_channel_zero(self):
        payload = {"channel": "2", "origin_channel": "0", "audience": "*"}
        self.assertEqual(DEMUX.delivery_address_label(payload), "0 (all audience)")

    def test_mcp_describes_projected_address_without_reinterpreting_raw_fields(self):
        payload = {
            "channel": "2", "origin_channel": "0", "audience": "*",
            "addressed_to": "3 (lena)", "text": "Exact projected message",
        }
        self.assertEqual(MCP.addressing(payload), "channel 3 (lena)")
        self.assertIn("on channel 3 (lena)", MCP.describe_delivery(payload))


if __name__ == "__main__":
    unittest.main()
