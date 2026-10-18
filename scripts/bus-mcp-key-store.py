"""Scoped native Codescribe event-key backend, one private-FD child per operation.

No listener, daemon, stdout secrets, shell, environment credentials or API bundle.
The executable and its hash are local reviewed bootstrap inputs, never MCP args.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import stat
import tempfile
import subprocess
import time


class NativeKeyError(RuntimeError):
    pass


class CodescribeSecretStore:
    codescribe_authority = True

    def __init__(self, executable, namespace, executable_sha256, *, timeout=10):
        self.executable = Path(executable).resolve(strict=True)
        if (not self.executable.is_file() or not os.access(self.executable, os.X_OK)
                or not re.fullmatch(r'[0-9a-f]{64}', executable_sha256)
                or not re.fullmatch(r'MCP_EVENTS_[0-9a-f]{64}_', namespace)
                or not .01 <= timeout <= 10):
            raise NativeKeyError('Reviewed native event-key binding required')
        self.namespace, self.digest, self.timeout = namespace, executable_sha256, timeout
        self.image = self.snapshot_executable()

    def close(self):
        image = getattr(self, 'image', None)
        if image is not None:
            image.close()
        directory = getattr(self, 'image_directory', None)
        if directory is not None:
            directory.cleanup()

    def __del__(self):
        self.close()

    def snapshot_executable(self):
        # macOS cannot exec /dev/fd. Pin reviewed bytes in our private directory
        # instead of resolving the deployment pathname again at exec. No writer
        # handle survives verification; only this binding owns the copy's path.
        directory = tempfile.TemporaryDirectory(prefix='codescribe-event-helper-')
        try:
            target = Path(directory.name) / 'image'
            source_fd = os.open(self.executable, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
            with os.fdopen(source_fd, 'rb') as source:
                mode = os.fstat(source.fileno()).st_mode
                if not stat.S_ISREG(mode) or not mode & 0o111:
                    raise NativeKeyError('Reviewed native executable required')
                output_fd = os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_CLOEXEC, 0o500)
                digest = hashlib.sha256()
                with os.fdopen(output_fd, 'wb') as output:
                    while chunk := source.read(1024 * 1024):
                        digest.update(chunk)
                        output.write(chunk)
                if digest.hexdigest() != self.digest:
                    raise NativeKeyError('Native executable changed')
                image = target.open('rb')
                if hashlib.file_digest(image, 'sha256').hexdigest() != self.digest:
                    image.close()
                    raise NativeKeyError('Native executable snapshot refused')
                image.seek(0)
            self.image_directory, self.image_path = directory, target
            return image
        except BaseException:
            directory.cleanup()
            raise

    def check_executable(self):
        with self.executable.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        if digest != self.digest:
            raise NativeKeyError('Native executable changed')

    def reference(self, value):
        if not isinstance(value, str) or not re.fullmatch(re.escape(self.namespace)+r'[0-9a-f]{32}', value):
            raise NativeKeyError('Signing-key namespace refused')
        return value

    def exchange(self, op, **values):
        request = {'version': 1, 'op': op, 'namespace': self.namespace, **values}
        raw = json.dumps(request, separators=(',', ':')).encode()
        if len(raw) > 2048:
            raise NativeKeyError('Bounded native request required')
        process = None
        parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        deadline = time.monotonic() + self.timeout
        try:
            self.check_executable()
            if self.image.closed:
                raise NativeKeyError('Native binding already closed')
            # Only non-secret process metadata; no inherited model, speech or
            # transport credentials.
            environment = {k: os.environ[k] for k in
                ('PATH', 'HOME', 'LANG', 'LC_ALL', 'TMPDIR', 'CODESCRIBE_DISABLE_KEYCHAIN') if k in os.environ}
            process = subprocess.Popen([str(self.image_path), 'mcp-event-key', '--fd', str(child.fileno())],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                pass_fds=(child.fileno(),), close_fds=True, env=environment)
            child.close()
            parent.settimeout(max(.001, deadline-time.monotonic()))
            parent.sendall(raw)
            parent.shutdown(socket.SHUT_WR)
            chunks, total = [], 0
            while True:
                remaining = deadline-time.monotonic()
                if remaining <= 0:
                    raise TimeoutError()
                parent.settimeout(remaining)
                chunk = parent.recv(min(4096, 32769-total))
                if not chunk:
                    break
                total += len(chunk)
                if total > 32768:
                    raise ValueError()
                chunks.append(chunk)
            process.wait(timeout=max(.001, deadline-time.monotonic()))
            if process.returncode != 0:
                raise ValueError()
            result = json.loads(b''.join(chunks))
            expected = {'ok', 'secret'} if op == 'get' else {'ok', 'references'} if op == 'list' else {'ok'}
            if not isinstance(result, dict) or result.get('ok') is not True or set(result) != expected:
                raise ValueError()
            return result
        except (OSError, ValueError, subprocess.SubprocessError):
            raise NativeKeyError('Native event-key operation refused') from None
        finally:
            parent.close(); child.close()
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait(timeout=1)

    def put(self, reference, secret):
        self.exchange('put', reference=self.reference(reference), secret=secret)

    def get(self, reference):
        result = self.exchange('get', reference=self.reference(reference))['secret']
        if not isinstance(result, str) or len(result) > 100:
            raise NativeKeyError('Native event-key response refused')
        return result

    def delete(self, reference):
        self.exchange('delete', reference=self.reference(reference))

    def references(self, namespace):
        if namespace != self.namespace:
            raise NativeKeyError('Signing-key namespace refused')
        refs = self.exchange('list')['references']
        if not isinstance(refs, list) or len(refs) > 128:
            raise NativeKeyError('Native event-key response refused')
        return [self.reference(r) for r in refs]
