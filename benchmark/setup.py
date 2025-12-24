#!/usr/bin/env python3
"""
Setup script for gabb-benchmark.

This script automates the full setup process:
1. Build gabb binary for Linux (cross-compile if needed)
2. Pull required Docker images
3. Verify environment

Run this before running benchmarks:
    python setup.py
"""

from __future__ import annotations

import argparse
import logging
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Optional

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%H:%M:%S",
)
logger = logging.getLogger(__name__)


# Paths
BENCHMARK_DIR = Path(__file__).parent
PROJECT_ROOT = BENCHMARK_DIR.parent
TARGET_DIR = PROJECT_ROOT / "target"
LINUX_BINARY_DIR = BENCHMARK_DIR / "bin"

# Try to load optional dependencies (may not be installed yet)
try:
    from dotenv import load_dotenv
    load_dotenv(BENCHMARK_DIR / ".env")
except ImportError:
    pass  # Will be available after install_python_deps()

try:
    import docker
except ImportError:
    docker = None  # Will be available after install_python_deps()


def setup_docker_host() -> None:
    """Auto-detect Docker socket on macOS if DOCKER_HOST not set."""
    if os.environ.get("DOCKER_HOST"):
        return  # Already set

    if platform.system() == "Darwin":
        # macOS Docker Desktop socket locations
        socket_paths = [
            Path.home() / ".docker/run/docker.sock",
            Path("/var/run/docker.sock"),
        ]
        for sock in socket_paths:
            if sock.exists():
                os.environ["DOCKER_HOST"] = f"unix://{sock}"
                logger.debug(f"Auto-detected Docker socket: {sock}")
                return


# Auto-detect Docker socket
setup_docker_host()


def check_rust_toolchain() -> bool:
    """Check if Rust toolchain is installed."""
    try:
        result = subprocess.run(
            ["rustc", "--version"],
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            logger.info(f"Rust toolchain found: {result.stdout.strip()}")
            return True
    except FileNotFoundError:
        pass

    logger.error("Rust toolchain not found. Install from https://rustup.rs/")
    return False


def check_docker() -> bool:
    """Check if Docker is available and running."""
    if docker is None:
        logger.error("Docker SDK not installed. Run: pip install docker")
        return False
    try:
        client = docker.from_env()
        client.ping()
        logger.info("Docker is available and running")
        return True
    except Exception as e:
        logger.error(f"Docker not available: {e}")
        return False


def check_cross_compilation_target() -> bool:
    """Check if Linux cross-compilation target is installed."""
    if platform.system() == "Linux":
        return True  # No cross-compilation needed

    # Check for x86_64-unknown-linux-musl target
    result = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        capture_output=True,
        text=True,
    )

    targets = result.stdout.strip().split("\n")
    linux_targets = [t for t in targets if "linux" in t]

    if linux_targets:
        logger.info(f"Linux targets installed: {linux_targets}")
        return True

    logger.warning("No Linux cross-compilation target found")
    return False


def install_cross_compilation_target() -> bool:
    """Install Linux cross-compilation target."""
    logger.info("Installing Linux cross-compilation target...")

    # For static linking, use musl
    target = "x86_64-unknown-linux-musl"

    result = subprocess.run(
        ["rustup", "target", "add", target],
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        logger.error(f"Failed to install target: {result.stderr}")
        return False

    logger.info(f"Installed target: {target}")
    return True


def build_gabb_native(release: bool = True) -> Path | None:
    """Build gabb for the native platform."""
    logger.info(f"Building gabb (release={release})...")

    cmd = ["cargo", "build"]
    if release:
        cmd.append("--release")

    result = subprocess.run(cmd, cwd=PROJECT_ROOT, capture_output=True, text=True)

    if result.returncode != 0:
        logger.error(f"Build failed: {result.stderr}")
        return None

    build_type = "release" if release else "debug"
    binary_path = TARGET_DIR / build_type / "gabb"

    if not binary_path.exists():
        logger.error(f"Binary not found at {binary_path}")
        return None

    logger.info(f"Native build complete: {binary_path}")
    return binary_path


def build_gabb_for_linux_docker() -> Path | None:
    """
    Build gabb for Linux using Docker.

    This is the most reliable cross-compilation method.
    """
    logger.info("Building gabb for Linux using Docker...")

    client = docker.from_env()

    # Pull the rust image if needed (use latest stable for Cargo.lock v4 support)
    rust_image = "rust:1.83-slim"
    try:
        client.images.get(rust_image)
    except docker.errors.ImageNotFound:
        logger.info(f"Pulling {rust_image} image...")
        client.images.pull(rust_image)

    # Create output directory
    LINUX_BINARY_DIR.mkdir(parents=True, exist_ok=True)

    # Build script
    build_script = """
    set -e
    cd /src
    cargo build --release
    cp target/release/gabb /output/gabb
    chmod +x /output/gabb
    """

    # Run build in container
    try:
        container = client.containers.run(
            image=rust_image,
            command=["bash", "-c", build_script],
            volumes={
                str(PROJECT_ROOT.absolute()): {"bind": "/src", "mode": "rw"},
                str(LINUX_BINARY_DIR.absolute()): {"bind": "/output", "mode": "rw"},
            },
            remove=True,
            detach=False,
            stdout=True,
            stderr=True,
        )
        logger.debug(f"Build output: {container}")
    except docker.errors.ContainerError as e:
        logger.error(f"Docker build failed: {e}")
        return None

    linux_binary = LINUX_BINARY_DIR / "gabb"
    if not linux_binary.exists():
        logger.error("Linux binary not found after build")
        return None

    logger.info(f"Linux build complete: {linux_binary}")
    return linux_binary


def build_gabb_for_linux_cross() -> Path | None:
    """
    Build gabb for Linux using cross-compilation.

    Requires musl-cross toolchain on macOS.
    """
    if platform.system() == "Linux":
        # Just do a native build
        return build_gabb_native(release=True)

    logger.info("Building gabb for Linux using cross-compilation...")

    target = "x86_64-unknown-linux-musl"

    # Check if cross is installed
    cross_available = shutil.which("cross") is not None

    if cross_available:
        # Use cross for easier cross-compilation
        cmd = ["cross", "build", "--release", "--target", target]
    else:
        # Direct cargo build (requires linker setup)
        cmd = ["cargo", "build", "--release", "--target", target]

    result = subprocess.run(cmd, cwd=PROJECT_ROOT, capture_output=True, text=True)

    if result.returncode != 0:
        logger.warning(f"Cross-compilation failed: {result.stderr}")
        logger.info("Falling back to Docker build...")
        return build_gabb_for_linux_docker()

    binary_path = TARGET_DIR / target / "release" / "gabb"

    if not binary_path.exists():
        logger.warning(f"Binary not found at {binary_path}, falling back to Docker build")
        return build_gabb_for_linux_docker()

    # Copy to benchmark bin directory
    LINUX_BINARY_DIR.mkdir(parents=True, exist_ok=True)
    dest = LINUX_BINARY_DIR / "gabb"
    shutil.copy2(binary_path, dest)

    logger.info(f"Linux build complete: {dest}")
    return dest


def pull_docker_images() -> bool:
    """Pull required Docker images."""
    logger.info("Pulling Docker images...")

    client = docker.from_env()
    images = ["python:3.11-slim"]

    for image in images:
        try:
            logger.info(f"Pulling {image}...")
            client.images.pull(image)
        except Exception as e:
            logger.error(f"Failed to pull {image}: {e}")
            return False

    logger.info("All images pulled successfully")
    return True


def install_python_deps() -> bool:
    """Install Python dependencies."""
    logger.info("Installing Python dependencies...")

    requirements_file = BENCHMARK_DIR / "requirements.txt"
    if not requirements_file.exists():
        logger.error(f"requirements.txt not found at {requirements_file}")
        return False

    # Install from requirements.txt
    result = subprocess.run(
        [sys.executable, "-m", "pip", "install", "-r", str(requirements_file)],
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        logger.error(f"Failed to install dependencies: {result.stderr}")
        return False

    logger.info("Python dependencies installed")
    return True


def verify_setup() -> dict:
    """Verify the setup is complete."""
    results = {
        "rust_toolchain": check_rust_toolchain(),
        "docker": check_docker(),
        "linux_binary": (LINUX_BINARY_DIR / "gabb").exists(),
        "native_binary": (TARGET_DIR / "release" / "gabb").exists(),
    }

    return results


def print_setup_status(results: dict) -> None:
    """Print setup status summary."""
    print("\n" + "=" * 50)
    print("Setup Status")
    print("=" * 50)

    for item, status in results.items():
        status_str = "OK" if status else "MISSING"
        status_color = "32" if status else "31"  # Green or Red
        print(f"  {item:20} [{status_str}]")

    all_ok = all(results.values())
    if all_ok:
        print("\nSetup complete. Ready to run benchmarks.")
    else:
        print("\nSetup incomplete. Run 'python setup.py' to fix issues.")

    print("=" * 50 + "\n")


def main():
    """Main setup function."""
    parser = argparse.ArgumentParser(description="Setup gabb-benchmark environment")
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="Skip building gabb binary",
    )
    parser.add_argument(
        "--skip-docker",
        action="store_true",
        help="Skip pulling Docker images",
    )
    parser.add_argument(
        "--skip-deps",
        action="store_true",
        help="Skip installing Python dependencies",
    )
    parser.add_argument(
        "--verify-only",
        action="store_true",
        help="Only verify setup, don't install anything",
    )
    parser.add_argument(
        "-v", "--verbose",
        action="store_true",
        help="Enable verbose logging",
    )

    args = parser.parse_args()

    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    print("\n" + "=" * 50)
    print("gabb-benchmark Setup")
    print("=" * 50 + "\n")

    # Verify only mode
    if args.verify_only:
        results = verify_setup()
        print_setup_status(results)
        return 0 if all(results.values()) else 1

    # Check Rust first (doesn't need Python deps)
    if not check_rust_toolchain():
        logger.error("Please install Rust first: https://rustup.rs/")
        return 1

    # Install Python deps FIRST so docker SDK is available
    if not args.skip_deps:
        if not install_python_deps():
            logger.error("Failed to install Python dependencies")
            return 1
        # Reload docker module after installing
        global docker
        try:
            import docker as docker_module
            docker = docker_module
        except ImportError:
            logger.error("Docker SDK failed to import after install")
            return 1

    # Now check Docker (needs SDK installed)
    if not check_docker():
        logger.error("Please install and start Docker first")
        return 1

    # Build gabb
    if not args.skip_build:
        # Build native binary
        native_binary = build_gabb_native(release=True)
        if not native_binary:
            logger.error("Native build failed")
            return 1

        # Build Linux binary for Docker
        linux_binary = build_gabb_for_linux_docker()
        if not linux_binary:
            logger.error("Linux build failed")
            return 1

    # Pull Docker images
    if not args.skip_docker:
        if not pull_docker_images():
            logger.error("Failed to pull Docker images")
            return 1

    # Verify
    results = verify_setup()
    print_setup_status(results)

    return 0 if all(results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
