"""Compatibility alias so `import backend` maps to `vistascribe.backend`."""

import sys

from vistascribe import backend as _backend

sys.modules[__name__] = _backend
