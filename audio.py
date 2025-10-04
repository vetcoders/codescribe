# audio.py
#
# purpose: handles audio recording from the default microphone. includes
#          functionality for starting/stopping recording, detecting periods
#          of silence to automatically stop recording, and saving the captured
#          audio to a temporary wav file suitable for transcription.
#
# dependencies: sounddevice (audio i/o)
#               numpy (numerical operations for audio data, rms calculation)
#               wave (saving audio to .wav format)
#               tempfile (creating temporary files)
#               asyncio (for running the collection loop concurrently)
#
# key components: recorder class
#                 start method (initializes and starts the audio stream)
#                 _collect method (async task to read audio chunks, detect silence)
#                 stop method (stops stream, saves buffer to temp wav file)
#                 sr, ch, silence_db, hang (constants for sample rate, channels,
#                                            silence threshold, hang time)
#
# design rationale: uses sounddevice for cross-platform audio input. silence
#                   detection is based on root mean square (rms) of audio chunks
#                   compared to a db threshold. asyncio is used in _collect to
#                   avoid blocking while waiting for audio data. saving to a temp
#                   file simplifies passing audio data to the transcription api.
import asyncio
import logging
import os
import tempfile
import wave

import numpy as np
import sounddevice as sd

# --- constants ---

# sample rate (samples per second)
# 16khz is standard for whisper
SR = 16000
# number of channels
# 1 for mono
CH = 1
# silence threshold in db
# rms values below this are considered silence.
# adjust this based on microphone sensitivity and background noise.
SILENCE_DB = -45
# silence duration threshold (seconds)
# recording stops automatically after this duration of continuous silence.
HANG = 0.8
# size of audio chunks to read from stream (samples)
BLOCK_SIZE = 1024

logging.basicConfig(level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s")

# --- recorder class ---


class Recorder:
    """handles audio recording, buffering, silence detection, and file saving.

    attributes:
        _buf (list): holds chunks of numpy audio data (int16).
        _stream (sounddevice.inputstream): the active audio input stream.
        _collect_task (asyncio.task): the task running the _collect loop.
    """

    def __init__(self):
        """initializes the recorder with empty buffer and no stream."""
        self._buf = []
        self._stream = None
        self._collect_task = None
        logging.info("Recorder initialized.")
        # Log default device info at initialization
        try:
            default_device_info = sd.query_devices(kind="input")
            name = default_device_info.get("name")
            index = default_device_info.get("index")
            max_ch = default_device_info.get("max_input_channels")
            logging.info(
                "Default input: name='%s', index=%s, max_ch=%s", name, index, max_ch
            )
        except Exception as e:
            logging.error(f"Could not query default input device: {e}")

    async def start(self):
        """starts the audio recording process.

        clears the buffer, creates and starts a new input stream,
        and launches the asynchronous _collect task to read audio data.
        raises assertionerror if recording is already in progress.
        """
        assert self._stream is None, "Recording is already in progress."
        logging.info("Starting recording...")
        self._buf = []
        try:
            # Try to get the system's default input device
            device_info = sd.query_devices(kind="input")
            device_index = device_info["index"]
            logging.info(
                "Using default input device: name='%s', index=%s",
                device_info.get("name"),
                device_index,
            )

            # create input stream with more robust settings
            self._stream = sd.InputStream(
                device=device_index,
                samplerate=SR,
                channels=CH,
                dtype="int16",
                blocksize=BLOCK_SIZE,
                latency="low",
            )
            self._stream.start()  # start the audio stream
            logging.info(f"Audio stream started (samplerate={SR}, channels={CH})")

            # start the background task to collect audio data
            self._collect_task = asyncio.create_task(self._collect())
            logging.info("Audio collection task created.")

        except Exception as e:
            logging.error(f"Failed to start audio stream: {e}")
            # clean up if stream creation failed partially
            if self._stream:
                if self._stream.active:
                    self._stream.stop()
                self._stream.close()
            self._stream = None
            # re-raise or handle appropriately
            raise

    async def _collect(self):
        """asynchronously collects audio data from the stream.

        runs in a loop, reading audio blocks, calculating rms volume,
        and checking for silence to automatically stop recording.
        this task is started by `start()` and cancelled by `stop()`.
        """
        silent_frames = 0  # counter for consecutive silent frames
        total_frames_processed = 0
        logging.info("Audio collection loop started.")

        try:
            while self._stream and self._stream.active:
                # wait for the next block non-blockingly
                await asyncio.sleep(0)  # yield control briefly

                # read a block of audio data
                # data is numpy array, status indicates overflows/underflows
                block, status = self._stream.read(BLOCK_SIZE)
                if status:
                    logging.warning(f"Sounddevice stream status: {status}")

                if len(block) == 0:
                    # stream might have closed unexpectedly or read timed out
                    logging.warning("Read 0 frames from audio stream.")
                    continue  # or break, depending on desired behavior

                self._buf.append(block.copy())  # append copy to buffer
                total_frames_processed += len(block)

                # calculate rms volume in dbfs (decibels relative to full scale)
                # add epsilon to avoid log10(0)
                # convert int16 to float for calculation
                rms_amplitude = np.sqrt(np.mean(block.astype(np.float32) ** 2))
                rms_db = 20 * np.log10(rms_amplitude + 1e-9)  # add epsilon for stability

                # check for silence
                if rms_db < SILENCE_DB:
                    # accumulate silent samples
                    silent_frames += len(block)
                else:
                    silent_frames = 0  # reset counter if sound detected
                    # print(f"Debug: Sound (RMS: {rms_db:.2f} dBFS)")

                # check if silence duration exceeds hang time
                if silent_frames / SR > HANG:
                    logging.info(f"Silence detected for > {HANG}s. Stopping collection.")
                    break  # exit collection loop

        except sd.PortAudioError as pae:
            logging.error(f"PortAudio error during collection: {pae}")
            # potentially try to gracefully stop/close stream here
        except Exception as e:
            logging.error(f"Error during audio collection: {e}", exc_info=True)
        finally:
            logging.info(
                f"Audio collection loop finished. Total frames processed: {total_frames_processed}"
            )
            # ensure stream stop is called even if loop breaks unexpectedly
            # this might be handled better in the main stop() method
            # if self._stream and self._stream.active:
            #     await self.stop() # careful with recursive calls or state issues

    async def stop(self) -> str | None:
        """stops the audio recording and saves the buffer to a temp wav file.

        stops and closes the audio stream, cancels the collection task,
        concatenates the buffered audio chunks, and writes them to a
        temporary .wav file.

        returns:
            str | none: the absolute path to the saved .wav file, or none if
                        no audio was recorded or an error occurred.
        """
        if not self._stream:
            logging.warning("Stop called but no active stream.")
            return None

        logging.info("Stopping recording...")
        try:
            # stop and close the audio stream
            if self._stream.active:
                self._stream.stop()
            self._stream.close()
            logging.info("Audio stream stopped and closed.")

            # cancel the collection task if it's running
            if self._collect_task and not self._collect_task.done():
                self._collect_task.cancel()
                try:
                    await self._collect_task  # allow task to process cancellation
                except asyncio.CancelledError:
                    logging.info("Audio collection task cancelled successfully.")
                except Exception as e:
                    logging.error(f"Error awaiting cancelled collection task: {e}")

            self._stream = None
            self._collect_task = None

            # check if any audio data was captured
            if not self._buf:
                logging.warning("No audio data captured.")
                return None

            # combine audio chunks
            wav_data = np.concatenate(self._buf)
            logging.info(
                f"Concatenated audio buffer: {len(wav_data)} frames ({len(wav_data) / SR:.2f}s)"
            )

            # create a temporary file (ensures unique name)
            # delete=false means the file persists after closing, we manage deletion later
            with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tf:
                path = tf.name
                logging.info(f"Saving audio to temporary file: {path}")

            # write data to the wav file
            try:
                with wave.open(path, "wb") as wf:
                    wf.setnchannels(CH)  # mono
                    wf.setsampwidth(2)  # 2 bytes for int16
                    wf.setframerate(SR)  # 16khz sample rate
                    wf.writeframes(wav_data.tobytes())  # write numpy array as bytes
                logging.info("Audio successfully saved to WAV file.")
                return path
            except Exception as e:
                logging.error(f"Failed to write WAV file at {path}: {e}")
                # attempt to clean up the temp file if writing failed
                try:
                    if os.path.exists(path):
                        os.remove(path)
                except OSError as ose:
                    logging.error(f"Error removing failed temp file {path}: {ose}")
                return None

        except sd.PortAudioError as pae:
            logging.error(f"PortAudio error during stream stop/close: {pae}")
            return None  # indicate failure
        except Exception as e:
            logging.error(f"Error stopping recording: {e}", exc_info=True)
            # ensure stream is reset even on error
            self._stream = None
            self._collect_task = None
            return None  # indicate failure
        finally:
            # clean buffer regardless of outcome
            self._buf = []
