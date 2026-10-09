"""One-click registration of skill and MCP server with common agents."""

import shutil
import subprocess
import sys
from pathlib import Path

from linux_x11_harness import get_binary_path, get_skill_path


def _log(msg: str) -> None:
    print(f"\033[0;32m[setup]\033[0m {msg}")


def _info(msg: str) -> None:
    print(f"\033[0;34m[info]\033[0m {msg}")


def _warn(msg: str) -> None:
    print(f"\033[1;33m[warn]\033[0m {msg}", file=sys.stderr)


def _err(msg: str) -> None:
    print(f"\033[0;31m[error]\033[0m {msg}", file=sys.stderr)


def _resolve_command() -> str:
    """Return the command agents should invoke to start the MCP proxy."""
    if cmd := shutil.which("linux-x11-harness"):
        return cmd
    binary = get_binary_path()
    if binary.exists():
        return str(binary)
    _err("linux-x11-harness not found in PATH and bundled binary is missing.")
    sys.exit(1)


def _link_skill(target_dir: Path) -> None:
    target_dir.mkdir(parents=True, exist_ok=True)
    link = target_dir / "linux-x11-harness"
    if link.is_symlink() or link.exists():
        link.unlink()
    link.symlink_to(get_skill_path(), target_is_directory=True)
    _log(f"Linked skill -> {link}")


def _run(cmd: list[str], check: bool = False) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        check=check,
    )


def _register_claude(command: str) -> None:
    if not shutil.which("claude"):
        _info("claude not found; skipping")
        return

    _link_skill(Path.home() / ".claude" / "skills")

    # Remove any existing registration to keep the config idempotent.
    _run(["claude", "mcp", "remove", "linux-x11-harness"])
    result = _run(["claude", "mcp", "add", "linux-x11-harness", "--", command])
    if result.returncode == 0:
        _log("Registered MCP server for Claude Code")
    else:
        _err(f"Failed to register Claude MCP server: {result.stderr}")


def _register_codex(command: str) -> None:
    if not shutil.which("codex"):
        _info("codex not found; skipping")
        return

    _link_skill(Path.home() / ".codex" / "skills")

    _run(["codex", "mcp", "remove", "linux-x11-harness"])
    result = _run(["codex", "mcp", "add", "linux-x11-harness", "--", command])
    if result.returncode == 0:
        _log("Registered MCP server for Codex")
    else:
        _err(f"Failed to register Codex MCP server: {result.stderr}")


def _register_kimi() -> None:
    if not shutil.which("kimi"):
        _info("kimi not found; skipping")
        return

    _link_skill(Path.home() / ".kimi-code" / "skills")
    _info("Kimi Code skill linked (MCP servers are not yet supported by kimi-code)")


def _register_qwen(command: str) -> None:
    if not shutil.which("qwen"):
        _info("qwen not found; skipping")
        return

    # Qwen stores project-scoped MCP settings in <cwd>/.qwen/settings.json.
    _run(["qwen", "mcp", "remove", "linux-x11-harness"])
    result = _run(["qwen", "mcp", "add", "linux-x11-harness", command])
    if result.returncode == 0:
        _log("Registered MCP server for Qwen")
    else:
        _err(f"Failed to register Qwen MCP server: {result.stderr}")


def _register_opencode(command: str) -> None:
    if not shutil.which("opencode"):
        _info("opencode not found; skipping")
        return

    # OpenCode has no CLI remove command; edit config directly to stay idempotent.
    config_path = Path.home() / ".config" / "opencode" / "opencode.json"
    if config_path.exists():
        import json

        try:
            data = json.loads(config_path.read_text())
            if "mcp" in data and "linux-x11-harness" in data["mcp"]:
                del data["mcp"]["linux-x11-harness"]
                config_path.write_text(json.dumps(data, indent=2))
        except (json.JSONDecodeError, OSError) as e:
            _warn(f"Could not clean up old OpenCode MCP entry: {e}")

    result = _run(["opencode", "mcp", "add", "linux-x11-harness", "--", command])
    if result.returncode == 0:
        _log("Registered MCP server for OpenCode")
    else:
        _err(f"Failed to register OpenCode MCP server: {result.stderr}")


def _register_pi() -> None:
    home = Path.home()
    if not shutil.which("pi") and not (home / ".pi").exists():
        _info("pi not found; skipping")
        return

    _link_skill(home / ".pi" / "agent" / "skills")


# (apt package name, binary name to check with shutil.which)
_REQUIRED_SYSTEM_PACKAGES = [
    ("xvfb", "Xvfb"),
    ("openbox", "openbox"),
    ("x11vnc", "x11vnc"),
    ("tigervnc-viewer", "vncviewer"),
    ("x11-utils", "xdpyinfo"),
]


def _check_system_deps() -> None:
    """Check and install required system packages via apt."""
    missing = [
        pkg for pkg, binary in _REQUIRED_SYSTEM_PACKAGES if not shutil.which(binary)
    ]

    if not missing:
        _log("All system dependencies present")
        return

    _warn(f"Missing system packages: {', '.join(missing)}")

    if not shutil.which("apt-get"):
        _err("Please install manually: sudo apt install " + " ".join(missing))
        sys.exit(1)

    _log(f"Installing: sudo apt install {' '.join(missing)}")
    result = _run(["sudo", "apt-get", "install", "-y", *missing])
    if result.returncode != 0:
        _err(f"Failed to install system packages: {result.stderr}")
        sys.exit(1)

    _log("System dependencies installed")


def setup_agents() -> None:
    """Detect common agents, install system deps, register skill + MCP server."""
    _check_system_deps()

    command = _resolve_command()
    _log(f"Using MCP command: {command}")

    _register_claude(command)
    _register_codex(command)
    _register_kimi()
    _register_qwen(command)
    _register_opencode(command)
    _register_pi()

    print()
    _log("Setup complete. Restart your agent to use linux-x11-harness.")
