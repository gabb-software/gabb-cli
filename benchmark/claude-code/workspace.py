"""
Workspace management for Claude Code benchmark.

Handles cloning repositories and checking out specific commits for SWE-bench tasks.
Uses a persistent cache to avoid re-cloning repos.
"""

from __future__ import annotations

import hashlib
import logging
import shutil
import subprocess
from pathlib import Path

logger = logging.getLogger(__name__)

# Default cache location
DEFAULT_CACHE_DIR = Path.home() / ".cache" / "gabb-benchmark" / "repos"


class WorkspaceManager:
    """Manages repository workspaces for benchmarking."""

    def __init__(self, cache_dir: Path | None = None):
        """
        Initialize the workspace manager.

        Args:
            cache_dir: Directory to cache cloned repositories.
                      Defaults to ~/.cache/gabb-benchmark/repos
        """
        self.cache_dir = cache_dir or DEFAULT_CACHE_DIR
        self.cache_dir.mkdir(parents=True, exist_ok=True)

    def get_workspace(
        self,
        repo: str,
        commit: str,
        force_fresh: bool = False,
    ) -> Path:
        """
        Get a workspace directory for a repo at a specific commit.

        Clones the repo if not cached, then checks out the specified commit.

        Args:
            repo: Repository in "owner/name" format (e.g., "scikit-learn/scikit-learn")
            commit: Git commit SHA to checkout
            force_fresh: If True, delete and re-clone even if cached

        Returns:
            Path to the workspace directory

        Raises:
            RuntimeError: If cloning or checkout fails
        """
        # Create a unique directory name for this repo
        repo_dir = self._get_repo_cache_dir(repo)

        # Clone if needed
        if force_fresh and repo_dir.exists():
            logger.info(f"Removing existing cache for {repo}")
            shutil.rmtree(repo_dir)

        if not repo_dir.exists():
            self._clone_repo(repo, repo_dir)

        # Create a workspace directory for this specific commit
        workspace_dir = self._get_commit_workspace(repo, commit)

        if not workspace_dir.exists():
            # Copy from cached repo and checkout
            self._create_commit_workspace(repo_dir, workspace_dir, commit)

        return workspace_dir

    def _get_repo_cache_dir(self, repo: str) -> Path:
        """Get the cache directory for a repository."""
        # Use repo name as directory (replace / with __)
        safe_name = repo.replace("/", "__")
        return self.cache_dir / safe_name

    def _get_commit_workspace(self, repo: str, commit: str) -> Path:
        """Get the workspace directory for a specific commit."""
        safe_name = repo.replace("/", "__")
        # Use first 12 chars of commit for directory name
        return self.cache_dir / "workspaces" / f"{safe_name}__{commit[:12]}"

    def _clone_repo(self, repo: str, target_dir: Path) -> None:
        """Clone a repository to the target directory."""
        url = f"https://github.com/{repo}.git"
        logger.info(f"Cloning {url} to {target_dir}")

        target_dir.parent.mkdir(parents=True, exist_ok=True)

        result = subprocess.run(
            ["git", "clone", "--depth", "1", "--no-single-branch", url, str(target_dir)],
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            # Try without depth limit for older commits
            result = subprocess.run(
                ["git", "clone", url, str(target_dir)],
                capture_output=True,
                text=True,
            )
            if result.returncode != 0:
                raise RuntimeError(f"Failed to clone {repo}: {result.stderr}")

        logger.info(f"Cloned {repo}")

    def _create_commit_workspace(
        self,
        repo_dir: Path,
        workspace_dir: Path,
        commit: str,
    ) -> None:
        """Create a workspace at a specific commit."""
        logger.info(f"Creating workspace at commit {commit[:12]}")

        workspace_dir.parent.mkdir(parents=True, exist_ok=True)

        # Copy the repo to workspace
        shutil.copytree(repo_dir, workspace_dir)

        # Fetch the specific commit if needed
        fetch_result = subprocess.run(
            ["git", "fetch", "origin", commit],
            cwd=workspace_dir,
            capture_output=True,
            text=True,
        )
        # Ignore fetch errors - commit might already be available

        # Checkout the commit
        result = subprocess.run(
            ["git", "checkout", commit],
            cwd=workspace_dir,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            # Try fetching all history then checkout
            subprocess.run(
                ["git", "fetch", "--unshallow"],
                cwd=workspace_dir,
                capture_output=True,
            )
            result = subprocess.run(
                ["git", "checkout", commit],
                cwd=workspace_dir,
                capture_output=True,
                text=True,
            )
            if result.returncode != 0:
                raise RuntimeError(f"Failed to checkout {commit}: {result.stderr}")

        logger.info(f"Workspace ready at {workspace_dir}")

    def cleanup_workspace(self, workspace_dir: Path) -> None:
        """Remove a workspace directory."""
        if workspace_dir.exists():
            # Remove gabb artifacts
            gabb_dir = workspace_dir / ".gabb"
            if gabb_dir.exists():
                shutil.rmtree(gabb_dir)

    def cleanup_all(self) -> None:
        """Remove all cached workspaces (keeps base repos)."""
        workspaces_dir = self.cache_dir / "workspaces"
        if workspaces_dir.exists():
            shutil.rmtree(workspaces_dir)
            logger.info("Cleaned up all workspaces")

    def list_cached_repos(self) -> list[str]:
        """List all cached repositories."""
        repos = []
        for path in self.cache_dir.iterdir():
            if path.is_dir() and path.name != "workspaces":
                repos.append(path.name.replace("__", "/"))
        return repos
