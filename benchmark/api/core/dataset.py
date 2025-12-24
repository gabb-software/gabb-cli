"""SWE-bench dataset loader and patch parser."""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Iterator

from datasets import load_dataset


@dataclass
class BenchmarkTask:
    """A single benchmarking task from SWE-bench."""

    instance_id: str
    repo: str
    base_commit: str
    problem_statement: str
    hints_text: str
    patch: str
    test_patch: str
    gold_files: list[str]

    @property
    def repo_url(self) -> str:
        """Get the GitHub clone URL for the repo."""
        return f"https://github.com/{self.repo}.git"

    @property
    def short_name(self) -> str:
        """Get a short display name for the task."""
        return self.instance_id


def parse_gold_files(patch_text: str) -> list[str]:
    """
    Extract the filenames modified in a git patch.

    This parses the unified diff format and extracts files from
    'diff --git a/path b/path' or '--- a/path' / '+++ b/path' lines.

    Args:
        patch_text: The raw patch text from SWE-bench.

    Returns:
        List of unique file paths modified by the patch.
    """
    files = set()

    # Pattern 1: diff --git a/path/to/file b/path/to/file
    diff_pattern = re.compile(r'^diff --git a/(.+?) b/(.+?)$', re.MULTILINE)
    for match in diff_pattern.finditer(patch_text):
        # Use the 'b' path (destination) as the canonical path
        files.add(match.group(2))

    # Pattern 2: --- a/path/to/file (source) and +++ b/path/to/file (dest)
    # This handles cases where diff --git line might be missing
    minus_pattern = re.compile(r'^--- a/(.+?)$', re.MULTILINE)
    plus_pattern = re.compile(r'^\+\+\+ b/(.+?)$', re.MULTILINE)

    for match in plus_pattern.finditer(patch_text):
        path = match.group(1)
        # Skip /dev/null for new files
        if path != '/dev/null':
            files.add(path)

    return sorted(files)


class SWEBenchDataset:
    """Loader for the SWE-bench_Verified dataset."""

    DATASET_NAME = "princeton-nlp/SWE-bench_Verified"

    def __init__(self, split: str = "test"):
        """
        Initialize the dataset loader.

        Args:
            split: Dataset split to load ('test' or 'train').
        """
        self.split = split
        self._dataset = None
        self._tasks_by_id: dict[str, BenchmarkTask] = {}

    def load(self) -> None:
        """Load the dataset from HuggingFace."""
        self._dataset = load_dataset(self.DATASET_NAME, split=self.split)
        self._build_task_index()

    def _build_task_index(self) -> None:
        """Build an index of tasks by instance_id."""
        for item in self._dataset:
            task = self._item_to_task(item)
            self._tasks_by_id[task.instance_id] = task

    def _item_to_task(self, item: dict) -> BenchmarkTask:
        """Convert a dataset item to a BenchmarkTask."""
        patch = item.get("patch", "")
        return BenchmarkTask(
            instance_id=item["instance_id"],
            repo=item["repo"],
            base_commit=item["base_commit"],
            problem_statement=item["problem_statement"],
            hints_text=item.get("hints_text", ""),
            patch=patch,
            test_patch=item.get("test_patch", ""),
            gold_files=parse_gold_files(patch),
        )

    def get_task(self, instance_id: str) -> BenchmarkTask | None:
        """
        Get a specific task by instance ID.

        Args:
            instance_id: The SWE-bench instance ID (e.g., 'scikit-learn__scikit-learn-10297').

        Returns:
            The BenchmarkTask or None if not found.
        """
        return self._tasks_by_id.get(instance_id)

    def iter_tasks(self, limit: int | None = None) -> Iterator[BenchmarkTask]:
        """
        Iterate over all tasks in the dataset.

        Args:
            limit: Maximum number of tasks to yield.

        Yields:
            BenchmarkTask instances.
        """
        count = 0
        for task in self._tasks_by_id.values():
            if limit is not None and count >= limit:
                break
            yield task
            count += 1

    def filter_by_repo(self, repo_pattern: str) -> list[BenchmarkTask]:
        """
        Filter tasks by repository name pattern.

        Args:
            repo_pattern: Substring to match in repo name.

        Returns:
            List of matching tasks.
        """
        return [
            task for task in self._tasks_by_id.values()
            if repo_pattern.lower() in task.repo.lower()
        ]

    def filter_by_language(self, extension: str) -> list[BenchmarkTask]:
        """
        Filter tasks by file extension in gold files.

        Args:
            extension: File extension to match (e.g., '.py', '.ts').

        Returns:
            List of tasks where gold files have the given extension.
        """
        return [
            task for task in self._tasks_by_id.values()
            if any(f.endswith(extension) for f in task.gold_files)
        ]

    @property
    def task_count(self) -> int:
        """Get the total number of tasks."""
        return len(self._tasks_by_id)

    @property
    def task_ids(self) -> list[str]:
        """Get all task instance IDs."""
        return list(self._tasks_by_id.keys())


# Convenience function for quick access
def load_swebench(split: str = "test") -> SWEBenchDataset:
    """
    Load the SWE-bench_Verified dataset.

    Args:
        split: Dataset split to load.

    Returns:
        Loaded SWEBenchDataset instance.
    """
    dataset = SWEBenchDataset(split=split)
    dataset.load()
    return dataset
