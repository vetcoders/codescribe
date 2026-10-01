import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("w0_measure", Path(__file__).parents[1] / "w0-measure.py")
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class W0MeasureTests(unittest.TestCase):
    def test_missing_is_unmeasured_and_cpu_is_in_cores(self):
        result = measure.summarize([{"cpu_percent": 150}, {"cpu_percent": 50}])
        self.assertEqual(result["cpu_cores"]["mean"], 1)
        self.assertIsNone(result["light_plus_total_ms"]["p95"])
        self.assertEqual(result["light_plus_total_ms"]["n"], 0)

    def test_percentile_and_memory_trend(self):
        self.assertEqual(measure.quantile(list(range(1, 101)), .95), 95)
        self.assertEqual(measure.summarize([
            {"elapsed_s": 0, "rss_bytes": 100}, {"elapsed_s": 2, "rss_bytes": 300}
        ])["rss_trend_bytes_per_s"], 100)

    def test_invalid_samples_do_not_become_measurements(self):
        for value in [float("nan"), float("inf"), -1, True, "16"]:
            with self.assertRaises(ValueError):
                measure.summarize([{"projection_main_ms": value}])


if __name__ == "__main__":
    unittest.main()
