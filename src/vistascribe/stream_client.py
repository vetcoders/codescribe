"""WebSocket streaming client for VistaScribeServer.

Replaces the legacy HTTP 'record-then-upload' flow with real-time streaming.
Connects to /ws/transcribe and handles protocol negotiation, chunk sending,
and event reception.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import uuid
from collections.abc import AsyncGenerator, Callable
from typing import Any

from websockets.exceptions import ConnectionClosed

from .client import resolve_server_url

logger = logging.getLogger(__name__)


class StreamingClient:
    def __init__(
        self,
        session_id: str | None = None,
        language: str | None = None,
        sample_rate: int = 16000,
        on_transcript: Callable[[str, bool], None] | None = None,
        on_error: Callable[[str], None] | None = None,
    ):
        self.session_id = session_id or uuid.uuid4().hex
        self.language = language
        self.sample_rate = sample_rate
        self.on_transcript = on_transcript
        self.on_error = on_error
        self._ws: Any = None
        self._url = None
        self._stop_event = asyncio.Event()
        self._send_queue: asyncio.Queue[bytes] = asyncio.Queue()

    async def connect(self) -> bool:
        """Establish WebSocket connection to the backend."""
        base_url = await resolve_server_url()
        if not base_url:
            if self.on_error:
                self.on_error("Backend server not found")
            return False

        # Convert http(s) to ws(s)
        ws_url = base_url.replace("http://", "ws://").replace("https://", "wss://")
        ws_url = f"{ws_url.rstrip('/')}/ws/transcribe"

        try:
            # Correct API usage: websockets.connect
            import websockets

            self._ws = await websockets.connect(ws_url)

            # Handshake / Config
            if self._ws:
                await self._ws.send(
                    json.dumps(
                        {
                            "type": "set",
                            "language": self.language,
                            "sample_rate": self.sample_rate,
                            "encoding": "pcm16",
                        }
                    )
                )

            logger.info(f"Connected to streaming backend: {ws_url} (sid={self.session_id})")
            return True
        except Exception as e:
            logger.error(f"Failed to connect to {ws_url}: {e}")
            if self.on_error:
                self.on_error(str(e))
            return False

    async def stream_audio(self, audio_generator: AsyncGenerator[bytes, None]):
        """Consume audio chunks from a generator and send them to the server."""
        if not self._ws:
            logger.error("Stream started without connection")
            return

        # Start receiver task
        receive_task = asyncio.create_task(self._receiver_loop())

        try:
            async for chunk in audio_generator:
                if self._stop_event.is_set():
                    break

                # Protocol: {"type": "chunk", "audio_base64": "..."}
                b64 = base64.b64encode(chunk).decode("ascii")
                msg = {"type": "chunk", "audio_base64": b64, "sample_rate": self.sample_rate}
                await self._ws.send(json.dumps(msg))

            # End of stream
            await self._ws.send(json.dumps({"type": "end"}))

            # Wait for final processing (receiver loop handles 'stream.closed' or closure)
            # We'll give it a moment or wait for the receive task to finish if desired.
            # For now, we let the receiver run until the server closes the socket.

        except Exception as e:
            logger.error(f"Streaming error: {e}")
            if self.on_error:
                self.on_error(f"Streaming error: {e}")
        finally:
            # Keep receiver alive for final transcript?
            # Usually the server sends 'transcript.final' then maybe closes or we close.
            await receive_task

    async def _receiver_loop(self):
        """Listen for messages from the server."""
        if not self._ws:
            return

        try:
            async for message in self._ws:
                try:
                    data = json.loads(message)
                except json.JSONDecodeError:
                    continue

                msg_type = data.get("type")

                if msg_type == "transcript.final":
                    text = data.get("text", "")
                    if self.on_transcript:
                        self.on_transcript(text, True)

                elif msg_type == "transcript.partial":
                    # Future-proofing: if backend supports partials
                    text = data.get("text", "")
                    if self.on_transcript:
                        self.on_transcript(text, False)

                elif msg_type == "error":
                    err = data.get("message", "Unknown server error")
                    logger.error(f"Server sent error: {err}")
                    if self.on_error:
                        self.on_error(err)

                elif msg_type == "stream.closed":
                    logger.info("Server closed stream cleanly")
                    break

        except ConnectionClosed:
            logger.info("WebSocket connection closed")
        except Exception as e:
            logger.error(f"Receiver loop error: {e}")
        finally:
            self._stop_event.set()

    async def close(self):
        """Force close connection."""
        self._stop_event.set()
        if self._ws:
            await self._ws.close()
