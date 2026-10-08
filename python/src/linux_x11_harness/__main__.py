"""CLI entry point: linux-x11-harness [mcp|serve|status|stop|setup]."""

import argparse
import subprocess
import sys

from linux_x11_harness import get_binary_path
from linux_x11_harness._install import setup_agents


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="linux-x11-harness",
        description="Linux X11 GUI harness with native automation driver.",
    )
    sub = parser.add_subparsers(dest="command")

    sub.add_parser("mcp", help="Run the MCP stdio proxy (default)")
    sub.add_parser("serve", help="Start the daemon")
    sub.add_parser("status", help="Check daemon status")
    sub.add_parser("stop", help="Stop the daemon")
    sub.add_parser("setup", help="Register skill and MCP server with detected agents")

    # Forward everything the wrapper does not know about (--socket PATH,
    # and future binary flags) straight to the native binary.
    args, extra = parser.parse_known_args()

    if args.command == "setup":
        setup_agents()
        return

    binary = get_binary_path()
    if not binary.exists():
        print(
            f"Binary not found: {binary}. "
            "Reinstall the package or run 'cargo build --release'.",
            file=sys.stderr,
        )
        sys.exit(1)

    cmd = [str(binary), args.command or "mcp", *extra]
    sys.exit(subprocess.run(cmd, stdin=sys.stdin, stdout=sys.stdout, stderr=sys.stderr).returncode)


if __name__ == "__main__":
    main()
