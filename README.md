<div align="center">

# linux-x11-harness

**Run Linux GUI apps in isolated X11 displays for AI agents.**

[![CI](https://github.com/Wangxiaoxiaoa/linux-x11-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/Wangxiaoxiaoa/linux-x11-harness/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[English](README.md) · [中文](README.zh-CN.md)

</div>

---

## What is this?

`linux-x11-harness` creates isolated X11 displays and lets AI agents control GUI applications. Each display gets its own X server, window manager, and application process group, so agents can click, type, capture screenshots, and read accessibility trees without touching the host desktop.

A read-only preview window opens on your desktop while the agent works, so you can watch every click and screenshot in real time.

Typical uses:

- Let an agent operate a GUI app without taking over your real desktop.
- Watch what the agent is doing live in a small preview window.
- Run end-to-end tests that need real input, focus, and screenshots.
- Capture accessibility trees or click UI elements programmatically.
- Discover installed/running apps and launch them by name.
- Invoke application menus by path and assert UI state after actions.

## Quick start

Install and register with your agents in one step:

```bash
pip install linux-x11-harness
linux-x11-harness setup
```

After setup, open your agent and start asking it to use GUI apps. The agent will launch the harness automatically when needed.

Requirements: Python 3.10+, Rust toolchain, Linux with `xvfb` and `openbox`.

## Supported agents

`linux-x11-harness setup` auto-configures both the MCP server and the agent skill where the agent supports them.

| Agent | MCP server | Agent skill |
|---|---|---|
| [Claude Code](https://claude.ai/code) | ✅ | ✅ |
| [Codex](https://github.com/openai/codex) | ✅ | ✅ |
| [Qwen](https://qwen.aliyun.com/) | ✅ | — |
| [OpenCode](https://opencode.ai/) | ✅ | — |
| [Kimi Code](https://kimi-code.moonshot.cn/) | — | ✅ |
| [Pi](https://pi.ai/) | — | ✅ |

Other MCP-compatible agents can connect manually with `linux-x11-harness mcp`.

## Manual commands

You normally do not need to run these. They are useful for debugging or running without an agent:

```bash
linux-x11-harness serve   # start the daemon
linux-x11-harness mcp     # run the MCP stdio proxy
linux-x11-harness status  # check daemon status
linux-x11-harness stop    # stop the daemon
```

## Documentation

- [Usage guide](docs/usage.md) — display previews, sockets, and multi-agent isolation.
- [Agent skill](skills/linux-x11-harness/SKILL.md) — quick reference for agents.
- [Architecture](docs/ARCHITECTURE.md) — crate layout and design notes.

## License

MIT
