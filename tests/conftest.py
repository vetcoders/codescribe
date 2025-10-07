# Ensure project root is importable during tests
import os
import sys

# Add the repository root to sys.path so tests can import top-level modules
REPO_ROOT = os.path.dirname(os.path.abspath(os.path.join(__file__, os.pardir)))
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)
