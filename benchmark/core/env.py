"""Docker environment wrapper for benchmarking."""

from __future__ import annotations

import asyncio
import hashlib
import logging
import os
import platform
import shutil
import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

import docker
from docker.models.containers import Container

from .dataset import BenchmarkTask

logger = logging.getLogger(__name__)


@dataclass
class CommandResult:
    """Result of executing a command in the container."""

    exit_code: int
    stdout: str
    stderr: str

    @property
    def success(self) -> bool:
        return self.exit_code == 0

    @property
    def output(self) -> str:
        """Combined stdout and stderr."""
        return f"{self.stdout}\n{self.stderr}".strip()


@dataclass
class EnvConfig:
    """Configuration for the benchmark environment."""

    # Docker settings
    image: str = "python:3.11-slim"
    memory_limit: str = "4g"
    cpu_count: int = 2

    # Paths
    gabb_binary_path: Path | None = None
    workspace_root: Path = field(default_factory=lambda: Path("/workspace"))

    # Timeouts
    clone_timeout: int = 300  # 5 minutes
    command_timeout: int = 60  # 1 minute
    index_timeout: int = 300  # 5 minutes for gabb indexing

    # Gabb settings
    gabb_db_path: str = "/workspace/.gabb/index.db"


class BenchmarkEnv:
    """
    Docker environment wrapper for running benchmarks.

    This class manages Docker containers for isolated benchmark execution.
    It handles:
    - Container lifecycle (create, start, stop, remove)
    - Binary mounting (gabb binary)
    - Repository cloning and checkout
    - Command execution
    """

    def __init__(self, config: EnvConfig | None = None):
        """
        Initialize the benchmark environment.

        Args:
            config: Environment configuration. Uses defaults if not provided.
        """
        self.config = config or EnvConfig()
        self._client: docker.DockerClient | None = None
        self._container: Container | None = None
        self._task: BenchmarkTask | None = None
        self._gabb_initialized = False

    @property
    def client(self) -> docker.DockerClient:
        """Get or create the Docker client."""
        if self._client is None:
            self._client = docker.from_env()
        return self._client

    @property
    def container(self) -> Container | None:
        """Get the current container."""
        return self._container

    @property
    def is_running(self) -> bool:
        """Check if the container is running."""
        if self._container is None:
            return False
        self._container.reload()
        return self._container.status == "running"

    async def setup(self, task: BenchmarkTask) -> None:
        """
        Set up the environment for a benchmark task.

        This creates a container, clones the repo, and checks out the base commit.

        Args:
            task: The benchmark task to set up.
        """
        self._task = task
        logger.info(f"Setting up environment for {task.instance_id}")

        # Create container
        await self._create_container(task)

        # Clone and checkout
        await self._clone_repo(task)
        await self._checkout_commit(task.base_commit)

        # Initialize gabb if binary is available
        if self.config.gabb_binary_path:
            await self._init_gabb()

    async def _create_container(self, task: BenchmarkTask) -> None:
        """Create and start the Docker container."""
        # Generate unique container name
        name_hash = hashlib.md5(task.instance_id.encode()).hexdigest()[:8]
        container_name = f"gabb-bench-{name_hash}"

        # Build volume mounts
        volumes = {}

        # Mount gabb binary if available
        if self.config.gabb_binary_path and self.config.gabb_binary_path.exists():
            volumes[str(self.config.gabb_binary_path.absolute())] = {
                "bind": "/usr/local/bin/gabb",
                "mode": "ro",
            }

        logger.debug(f"Creating container {container_name} with image {self.config.image}")

        # Create container
        self._container = self.client.containers.create(
            image=self.config.image,
            name=container_name,
            command="sleep infinity",
            volumes=volumes,
            mem_limit=self.config.memory_limit,
            cpu_count=self.config.cpu_count,
            working_dir=str(self.config.workspace_root),
            detach=True,
            tty=True,
        )

        # Start container
        self._container.start()
        logger.info(f"Container {container_name} started")

        # Install git
        result = await self.exec("apt-get update && apt-get install -y git", timeout=120)
        if not result.success:
            raise RuntimeError(f"Failed to install git: {result.stderr}")

        # Make gabb executable if mounted
        if self.config.gabb_binary_path:
            await self.exec("chmod +x /usr/local/bin/gabb")

    async def _clone_repo(self, task: BenchmarkTask) -> None:
        """Clone the repository into the container."""
        logger.info(f"Cloning {task.repo_url}")

        result = await self.exec(
            f"git clone --depth 100 {task.repo_url} {self.config.workspace_root}",
            timeout=self.config.clone_timeout,
        )

        if not result.success:
            raise RuntimeError(f"Failed to clone repo: {result.stderr}")

    async def _checkout_commit(self, commit: str) -> None:
        """Checkout a specific commit."""
        logger.info(f"Checking out commit {commit[:8]}")

        # Fetch the specific commit if needed
        await self.exec(f"git fetch --depth 1 origin {commit}", timeout=60)

        result = await self.exec(f"git checkout {commit}", timeout=30)
        if not result.success:
            raise RuntimeError(f"Failed to checkout commit: {result.stderr}")

    async def _init_gabb(self) -> None:
        """Initialize gabb indexing in the workspace."""
        if not self.config.gabb_binary_path:
            return

        logger.info("Initializing gabb index")

        # Start gabb daemon to index the workspace
        result = await self.exec(
            f"gabb daemon start --workspace {self.config.workspace_root} "
            f"--db {self.config.gabb_db_path}",
            timeout=self.config.index_timeout,
        )

        if result.success:
            self._gabb_initialized = True
            logger.info("Gabb indexing complete")
        else:
            logger.warning(f"Gabb init failed: {result.stderr}")

    async def exec(
        self,
        command: str,
        timeout: int | None = None,
        workdir: str | None = None,
    ) -> CommandResult:
        """
        Execute a command in the container.

        Args:
            command: The command to execute.
            timeout: Timeout in seconds. Uses config default if not provided.
            workdir: Working directory. Uses workspace root if not provided.

        Returns:
            CommandResult with exit code and output.
        """
        if not self.is_running:
            raise RuntimeError("Container is not running")

        timeout = timeout or self.config.command_timeout
        workdir = workdir or str(self.config.workspace_root)

        logger.debug(f"Executing: {command[:100]}...")

        # Run in thread to avoid blocking
        loop = asyncio.get_event_loop()
        result = await loop.run_in_executor(
            None,
            self._exec_sync,
            command,
            workdir,
        )

        return result

    def _exec_sync(self, command: str, workdir: str) -> CommandResult:
        """Synchronous command execution."""
        exit_code, output = self._container.exec_run(
            cmd=["bash", "-c", command],
            workdir=workdir,
            demux=True,
        )

        stdout = output[0].decode("utf-8", errors="replace") if output[0] else ""
        stderr = output[1].decode("utf-8", errors="replace") if output[1] else ""

        return CommandResult(exit_code=exit_code, stdout=stdout, stderr=stderr)

    async def cleanup(self) -> None:
        """Stop and remove the container."""
        if self._container:
            logger.info(f"Cleaning up container {self._container.name}")
            try:
                self._container.stop(timeout=5)
                self._container.remove(force=True)
            except Exception as e:
                logger.warning(f"Error during cleanup: {e}")
            finally:
                self._container = None
                self._gabb_initialized = False

    async def __aenter__(self) -> "BenchmarkEnv":
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        """Async context manager exit."""
        await self.cleanup()


class GabbBinaryBuilder:
    """Builder for the gabb binary."""

    def __init__(self, project_root: Path):
        """
        Initialize the builder.

        Args:
            project_root: Path to the gabb-cli project root.
        """
        self.project_root = project_root
        self.target_dir = project_root / "target"

    def get_binary_path(self, release: bool = True) -> Path:
        """
        Get the path to the built binary.

        Args:
            release: Whether to use release build.

        Returns:
            Path to the binary.
        """
        build_type = "release" if release else "debug"
        binary_name = "gabb"

        # Handle platform-specific binary name
        if platform.system() == "Windows":
            binary_name = "gabb.exe"

        return self.target_dir / build_type / binary_name

    def build(self, release: bool = True) -> Path:
        """
        Build the gabb binary.

        Args:
            release: Whether to build in release mode.

        Returns:
            Path to the built binary.

        Raises:
            RuntimeError: If build fails.
        """
        logger.info(f"Building gabb binary (release={release})")

        cmd = ["cargo", "build"]
        if release:
            cmd.append("--release")

        result = subprocess.run(
            cmd,
            cwd=self.project_root,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            raise RuntimeError(f"Build failed: {result.stderr}")

        binary_path = self.get_binary_path(release)
        if not binary_path.exists():
            raise RuntimeError(f"Binary not found at {binary_path}")

        logger.info(f"Build complete: {binary_path}")
        return binary_path

    def build_for_linux(self, release: bool = True) -> Path:
        """
        Cross-compile gabb for Linux (for Docker containers).

        This uses cross-compilation or builds inside a container.

        Args:
            release: Whether to build in release mode.

        Returns:
            Path to the Linux binary.
        """
        # Check if we're already on Linux
        if platform.system() == "Linux":
            return self.build(release)

        # For macOS, we need to cross-compile or use Docker
        logger.info("Cross-compiling for Linux using Docker")

        # Create a temporary build script
        build_script = """
        #!/bin/bash
        set -e
        apt-get update && apt-get install -y curl build-essential
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source $HOME/.cargo/env
        cd /src
        cargo build --release
        """

        # Run build in Docker container
        client = docker.from_env()

        # Create output directory
        output_dir = self.project_root / "target" / "linux-release"
        output_dir.mkdir(parents=True, exist_ok=True)

        container = client.containers.run(
            image="rust:1.75-slim",
            command=["bash", "-c", build_script],
            volumes={
                str(self.project_root.absolute()): {"bind": "/src", "mode": "rw"},
            },
            remove=True,
            detach=False,
        )

        # The binary should now be at target/release/gabb (Linux binary)
        linux_binary = self.target_dir / "release" / "gabb"
        if not linux_binary.exists():
            raise RuntimeError("Linux binary not found after cross-compilation")

        # Copy to linux-specific location
        dest = output_dir / "gabb"
        shutil.copy2(linux_binary, dest)

        logger.info(f"Linux binary ready: {dest}")
        return dest


def get_default_gabb_binary() -> Path | None:
    """
    Find the default gabb binary to use.

    Looks in common locations:
    1. Local release build
    2. Local debug build
    3. System PATH

    Returns:
        Path to the binary, or None if not found.
    """
    # Check project target directories
    project_root = Path(__file__).parent.parent.parent
    release_binary = project_root / "target" / "release" / "gabb"
    debug_binary = project_root / "target" / "debug" / "gabb"

    if release_binary.exists():
        return release_binary
    if debug_binary.exists():
        return debug_binary

    # Check system PATH
    gabb_path = shutil.which("gabb")
    if gabb_path:
        return Path(gabb_path)

    return None
