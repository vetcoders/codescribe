"""Native PCM queue contract without loading AudioToolbox or opening a device."""

import contextlib
import ctypes
import fcntl
import os
import queue
import tempfile
import time
from types import SimpleNamespace
from unittest.mock import patch
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
ActualPCMStreamPlayer = DEMUX.PCMStreamPlayer


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

    def test_short_aligned_stream_starts_at_eof(self):
        self.assertEqual(self.native.started, 0)
        self.player.enqueue(b"\x01")
        self.assertEqual(self.native.started, 0)
        self.player.enqueue(b"\x02\x03")
        self.player.enqueue(b"\x04")
        self.assertEqual(self.native.started, 0)
        self.player.finish()
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


class PCMStreamingBufferTests(unittest.TestCase):
    def player(self, native=None):
        native = native or AudioQueueFixture()
        player = ActualPCMStreamPlayer(_library=native)
        self.addCleanup(player.terminate)
        return player, native

    def wait_for(self, predicate):
        end = time.monotonic() + 2
        while not predicate() and time.monotonic() < end:
            time.sleep(.002)
        self.assertTrue(predicate())

    def test_prefill_uses_duration_and_deadline_not_fragment_count(self):
        player, native = self.player()
        for _ in range(2):
            player.enqueue(b"\0" * 4800)
        self.assertEqual(native.started, 0)
        player.enqueue(b"\0" * 4800)
        self.assertEqual(native.started, 1)
        self.assertEqual(player.diagnostics()["start_reserve_ms"], 300)
        other, api = self.player()
        with patch.object(DEMUX.time, "monotonic", return_value=10):
            other.enqueue(b"\1\2")
        with patch.object(DEMUX.time, "monotonic", return_value=10.299):
            other.start_if_ready()
            self.assertEqual(api.started, 0)
        with patch.object(DEMUX.time, "monotonic", return_value=10.301):
            other.start_if_ready()
            self.assertEqual(api.started, 1)

    def test_fragmentation_preserves_pcm_and_requires_native_drain(self):
        pcm = bytes(range(256)) * 90
        for size in [1, 3, 511, 16384]:
            with self.subTest(fragment=size):
                player, native = self.player()
                events = queue.Queue()
                pieces = [pcm[i:i + size] for i in range(0, len(pcm), size)]
                for piece in pieces[1:]:
                    events.put(("pcm", piece))
                events.put(("done", (True, None, None)))
                stream = SimpleNamespace(poll_audio=lambda: events.get_nowait() if not events.empty() else None)
                feed = DEMUX._PCMStreamFeed(stream, player, pieces[0])
                feed.start()
                self.addCleanup(feed.close)
                self.wait_for(feed.finished.is_set)
                self.assertEqual(feed.result, (True, None, None))
                self.assertEqual(b"".join(native.queued_bytes()), pcm)
                self.assertLessEqual(player.pending, player.MAX_PENDING)
                while native.enqueued:
                    native.consume()
                self.assertIsNone(player.poll(), "reusable buffers are not hardware drain")
                native.running = 0
                self.assertEqual(player.poll(), 0)

    def test_deadline_starts_without_another_transport_fragment(self):
        player, native = self.player()
        feed = DEMUX._PCMStreamFeed(SimpleNamespace(poll_audio=lambda: None), player, b"\1\2")
        feed.start()
        self.addCleanup(feed.close)
        self.wait_for(lambda: native.started == 1)
        self.assertEqual(native.queued_bytes(), [b"\1\2"])
        self.assertFalse(feed.finished.is_set())

    def test_full_native_queue_cancels_without_waiting_for_audio(self):
        player, native = self.player()
        events = queue.Queue()
        for _ in range(20):
            events.put(("pcm", b"\0" * 16384))
        stream = SimpleNamespace(poll_audio=lambda: events.get_nowait() if not events.empty() else None)
        feed = DEMUX._PCMStreamFeed(stream, player, b"\0" * 16384)
        feed.start()
        self.wait_for(lambda: player.pending == player.MAX_PENDING)
        feed.close()
        self.assertFalse(feed.thread.is_alive())
        self.assertEqual(player.poll(), -15)
        self.assertEqual(native.disposed, 1)
        starts = native.started
        with self.assertRaises(OSError):
            player.enqueue(b"\0\0")
        player.start_if_ready(force=True)
        self.assertEqual(native.started, starts)

    def test_cancelled_blocked_producer_is_bounded_and_cannot_publish_late_pcm(self):
        entered, release = threading.Event(), threading.Event()
        def synthesize(_text, emit):
            entered.set()
            release.wait(3)
            emit(b"\1\2")
            return b"\1\2", None, None
        stream = DEMUX.SpeechPCMStream(["text"], synthesize)
        stream.start()
        self.assertTrue(entered.wait(1))
        try:
            start = time.monotonic()
            self.assertEqual(stream.close(), "NOT_JOINED")
            self.assertLess(time.monotonic() - start, 1)
            self.assertTrue(stream.thread.is_alive())
        finally:
            release.set()
            stream.thread.join(1)
        self.assertFalse(stream.thread.is_alive())
        self.assertIsNone(stream.poll_audio())

    def test_full_producer_queue_is_bounded_and_joins_after_cancel(self):
        def synthesize(_text, emit):
            emit(b"\0" * 400000)
            return b"\0" * 400000, None, None
        stream = DEMUX.SpeechPCMStream(["text"], synthesize)
        stream.start()
        self.wait_for(lambda: stream.queue.qsize() == 8)
        self.assertEqual(stream.close(), "JOINED")
        self.assertFalse(stream.thread.is_alive())
        self.assertLessEqual(stream.queue.qsize(), 8)

    @contextlib.contextmanager
    def owner(self, root, native):
        with contextlib.ExitStack() as stack:
            for name, value in [
                ("_wait_for_take_end", True), ("installation_idle", True),
                ("_save_lifecycle_cursor", None), ("channel_quiet_for_playback", set()),
                ("channel_keeps_playback", set()),
            ]:
                stack.enter_context(patch.object(DEMUX, name, return_value=value))
            def announce(runtime, _duration):
                marker = runtime / "integrator-playback.json"
                marker.write_text("{}")
                return marker
            def withdraw(marker):
                self.assertGreaterEqual(native.disposed, 1)
                self.assertFalse(any(t.name == "reply-pcm-feed" and t.is_alive() for t in threading.enumerate()))
                marker.unlink()
            stack.enter_context(patch.object(DEMUX, "_announce_playback", side_effect=announce))
            stack.enter_context(patch.object(DEMUX, "_withdraw_playback", side_effect=withdraw))
            stack.enter_context(patch.object(DEMUX, "PCMStreamPlayer", side_effect=lambda: ActualPCMStreamPlayer(_library=native)))
            yield
        descriptor = os.open(root / "runtime/playback.lock", os.O_RDWR)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(descriptor)

    def test_owner_does_not_claim_success_for_an_unjoined_producer_after_native_drain(self):
        native = AudioQueueFixture()
        def drain(_queue, immediate):
            native.stops.append(bool(immediate))
            while native.enqueued:
                native.consume()
            native.running = 0
            return 0
        native.AudioQueueStop = NativeFunction(drain)
        stream = DEMUX.SpeechPCMStream([], lambda *_: None)
        class PendingProducer:
            ident = 123
            def start(self):
                stream.queue.put(("pcm", b"\0" * 14400))
                stream.queue.put(("done", (True, None, None)))
            def join(self, timeout):
                self.timeout = timeout
            def is_alive(self):
                return True
        producer = PendingProducer()
        stream.thread = producer
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.owner(root, native):
                result = DEMUX._play_pcm_24k(stream, playback_root=root, bus=root / "bus.jsonl")
        self.assertEqual(result[0], False)
        self.assertEqual(result[2], "playback_failed")
        self.assertIn("not joined", result[1])
        self.assertEqual(producer.timeout, .25)
        self.assertEqual(native.disposed, 1)


if __name__ == "__main__":
    unittest.main()
