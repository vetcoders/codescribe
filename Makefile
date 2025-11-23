.PHONY: all check lint format test security clean

# Default target: run all checks
all: check

# Format code using Ruff (replaces Black/Isort) - MODIFIES FILES
format:
	@echo "Running Ruff format..."
	@uv run ruff format .

# Lint code using Ruff and check types with Mypy - READ ONLY
lint:
	@echo "Running Ruff lint..."
	@uv run ruff check .
	@echo "Running Mypy..."
	@uv run mypy src/vistascribe

# Security scan using Bandit (only on source code, ignoring venv/tests)
security:
	@echo "Running Bandit..."
	@uv run bandit -ll -c pyproject.toml -r src/vistascribe

# Run tests using Pytest
test:
	@echo "Running Pytest..."
	@uv run pytest

# Run full pipeline (READ ONLY)
check: lint security test

# Clean up temporary files
clean:
	@rm -rf .pytest_cache .ruff_cache .mypy_cache
	@find . -name "__pycache__" -type d -exec rm -rf {} +
