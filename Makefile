.PHONY: all check lint format test security clean build install release

# Default target: run all checks
all: check

# Build Rust binary (debug)
build:
	@echo "Building Rust binary (debug)..."
	@cd codescribe-rs && cargo build

# Build Rust binary (release)
release:
	@echo "Building Rust binary (release)..."
	@cd codescribe-rs && cargo build --release

# Install Rust binary to ~/.cargo/bin (standard Rust location)
install:
	@echo "Installing CodeScribe to ~/.cargo/bin..."
	@cd codescribe-rs && cargo install --path .
	@echo "✅ Installed: ~/.cargo/bin/codescribe"

# Format code using Ruff (replaces Black/Isort) - MODIFIES FILES
format:
	@echo "Running Ruff format..."
	@uv run ruff format .

# Lint code using Ruff and check types with Mypy - READ ONLY
lint:
	@echo "Running Ruff lint..."
	@uv run ruff check .
	@echo "Running Mypy..."
	@uv run mypy src/codescribe

# Security scan using Bandit (only on source code, ignoring venv/tests)
security:
	@echo "Running Bandit..."
	@uv run bandit -ll -c pyproject.toml -r src/codescribe

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
