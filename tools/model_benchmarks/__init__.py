"""Secure model-benchmark snapshot updater."""

from .core import BenchmarkError, UpdateResult, update_benchmarks

__all__ = ["BenchmarkError", "UpdateResult", "update_benchmarks"]
