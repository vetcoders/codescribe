"""Native PCM queue contract without loading AudioToolbox or opening a device."""

import ctypes
import importlib.util
from pathlib import Path
import sys
import threading
import unittest


SPEC = importlib.util.spec_from_file_location(
    "bus_demux_pcm_player_test", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class NativeFunction:
    def __init__(self, implementation):
        self.implementation = implementation

    def __call__(self, *arguments):
        return self.implementation(*arguments)


class AudioQueueFixture:
    """Allocated buffer memory is real ctypes; all native operations are local."""

    def __init__(self):
        self.allocated = []
        self.enqueued = []
        self.started = 0
        self.stops = []
        self.disposed = 0
        self.running = 0
        self.status_failure = 0
        self.callback_during_dispose = False
        for name in ("NewOutput", "AllocateBuffer", "EnqueueBuffer", "Start", "Stop",
                     "Dispose", "GetProperty"):
            setattr(self, "AudioQueue" + name,
                    NativeFunction(getattr(self, "_" + name)))

    def _NewOutput(self, description, callback, _context, _loop, _mode, _flags, queue):
        self.description = description._obj
        self.callback = callback
        queue._obj.value = 71
        return 0

    def _AllocateBuffer(self, _queue, capacity, result):
        memory = ctypes.create_string_buffer(capacity)
        buffer = DEMUX._PCMQueueBuffer()
        buffer.capacity = capacity
        buffer.data = ctypes.cast(memory, ctypes.c_void_p)
        reference = ctypes.pointer(buffer)
        ctypes.cast(result, ctypes.POINTER(DEMUX._PCMQueueBufferRef))[0] = reference
        self.allocated.append((reference, memory))
        return 0

    def _EnqueueBuffer(self, _queue, buffer, _packets, _descriptions):
        self.enqueued.append(buffer)
        return 0

    def _Start(self, _queue, _timestamp):
        self.started += 1
        self.running = 1
        return 0

    def _Stop(self, _queue, immediate):
        self.stops.append(bool(immediate))
        return 0

    def _Dispose(self, _queue, immediate):
        if self.callback_during_dispose and self.enqueued:
            # Real Dispose can synchronously wait for its callback thread. A
            # condition held across Dispose would deadlock that native thread.
            callback = threading.Thread(target=self.consume)
            callback.start()
            callback.join(timeout=0.5)
            if callback.is_alive():
                raise AssertionError("Dispose blocked its callback thread")
        self.disposed += 1
        self.immediate_dispose = bool(immediate)
        return 0

    def _GetProperty(self, _queue, _property, result, _size):
        result._obj.value = self.running
        return self.status_failure

    def consume(self):
        buffer = self.enqueued.pop(0)
        self.callback(None, ctypes.c_void_p(71), buffer)

    def queued_bytes(self):
        return [ctypes.string_at(buffer.contents.data, buffer.contents.byte_size)
                for buffer in self.enqueued]


class PCMStreamPlayerTests(unittest.TestCase):
    def setUp(self):
        self.native = AudioQueueFixture()
        self.player = DEMUX.PCMStreamPlayer(_library=self.native)
        self.addCleanup(self.player.terminate)

    def test_starts_only_after_first_complete_pcm_sample(self):
        self.assertEqual(self.native.started, 0)
        self.player.enqueue(b"\x01")
        self.assertEqual(self.native.started, 0)
        self.player.enqueue(b"\x02\x03")
        self.player.enqueue(b"\x04")
        self.assertEqual(self.native.started, 1)
        self.assertEqual(self.native.queued_bytes(), [b"\x01\x02", b"\x03\x04"])
        self.assertEqual(self.native.description.sample_rate, 24000)
        self.assertEqual(self.native.description.bits_per_channel, 16)

    def test_pending_buffers_are_bounded_and_enqueue_does_not_wait(self):
        for _ in range(self.player.MAX_PENDING):
            self.assertTrue(self.player.can_enqueue())
            self.player.enqueue(b"\x00\x00")
        self.assertFalse(self.player.can_enqueue())
        self.assertEqual(self.player.pending, self.player.MAX_PENDING)
        with self.assertRaises(BufferError):
            self.player.enqueue(b"\x00\x00")

    def test_callback_reuses_buffers_without_freeing_running_queue_memory(self):
        self.player.enqueue(b"\x00\x00")
        self.native.consume()
        self.assertEqual(self.player.pending, 0)
        self.player.enqueue(b"\x01\x02")
        self.assertEqual(len(self.native.allocated), 1)
        self.assertEqual(self.native.queued_bytes(), [b"\x01\x02"])

    def test_finish_drains_hardware_before_reporting_success(self):
        self.player.enqueue(b"\x00\x00")
        self.player.finish()
        self.assertEqual(self.native.stops, [False])
        self.assertIsNone(self.player.poll())
        self.native.consume()
        self.assertIsNone(self.player.poll(), "buffer reuse is not hardware drain")
        self.native.running = 0
        self.assertEqual(self.player.poll(), 0)
        self.assertEqual(self.native.disposed, 1)
        self.assertEqual(self.player.poll(), 0)

    def test_empty_finished_stream_never_starts_audio(self):
        self.player.finish()
        self.assertEqual(self.player.poll(), 0)
        self.assertEqual(self.native.started, 0)

    def test_partial_sample_and_oversized_blocks_are_refused(self):
        with self.assertRaises(ValueError):
            self.player.enqueue(b"\x00" * (self.player.BLOCK_BYTES + 1))
        self.player.enqueue(b"\x01")
        with self.assertRaises(ValueError):
            self.player.finish()

    def test_termination_allows_dispose_callback_and_is_idempotent(self):
        self.player.enqueue(b"\x00\x00")
        self.native.callback_during_dispose = True
        self.player.terminate()
        self.assertEqual(self.player.poll(), -15)
        self.assertEqual(self.player.wait(timeout=0), -15)
        self.player.kill()
        self.player.terminate()
        self.assertEqual(self.native.disposed, 1)
        self.assertTrue(self.native.immediate_dispose)

    def test_native_status_failure_is_a_terminal_failure(self):
        self.player.enqueue(b"\x00\x00")
        self.player.finish()
        self.native.consume()
        self.native.status_failure = -50
        self.assertEqual(self.player.poll(), 1)
        self.assertIn("status failed", self.player.error)
        self.assertEqual(self.native.disposed, 1)

    def test_wait_timeout_and_closed_enqueue_match_process_control(self):
        with self.assertRaises(DEMUX.subprocess.TimeoutExpired):
            self.player.wait(timeout=0)
        self.player.terminate()
        with self.assertRaises(OSError):
            self.player.enqueue(b"\x00\x00")


if __name__ == "__main__":
    unittest.main()
