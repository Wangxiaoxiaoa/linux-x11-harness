<div align="center">

# linux-x11-harness

**为 AI 智能体提供隔离的 Linux X11 GUI 运行环境。**

[![CI](https://github.com/Wangxiaoxiaoa/linux-x11-harness/actions/workflows/ci.yml/badge.svg)](https://github.com/Wangxiaoxiaoa/linux-x11-harness/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[English](README.md) · [中文](README.zh-CN.md)

</div>

---

## 这是什么？

`linux-x11-harness` 可以创建隔离的 X11 显示环境，并让 AI 智能体控制 GUI 应用。每个显示环境都有独立的 X 服务器、窗口管理器和应用进程组，因此智能体可以点击、输入、截图、读取无障碍树，而不会影响宿主桌面。

智能体工作时，桌面上会自动打开一个只读预览小窗，让你实时看到它的每次点击和截图。

典型用途：

- 让智能体操作 GUI 应用，而不接管真实桌面。
- 通过小窗实时观看智能体的操作过程。
- 运行需要真实输入、焦点和截图的端到端测试。
- 捕获无障碍树或屏幕画面用于验证。

## 快速开始

一条命令完成安装并向各智能体注册：

```bash
pip install linux-x11-harness
linux-x11-harness setup
```

安装完成后，直接打开智能体并让它操作 GUI 应用即可。智能体需要时会自动启动 harness。

环境要求：Python 3.10+、Rust 工具链、已安装 `xvfb` 和 `openbox` 的 Linux 系统。

## 支持的智能体

`linux-x11-harness setup` 会在智能体支持的情况下，同时注册 MCP server 和 Agent skill。

| 智能体 | MCP server | Agent skill |
|---|---|---|
| [Claude Code](https://claude.ai/code) | ✅ | ✅ |
| [Codex](https://github.com/openai/codex) | ✅ | ✅ |
| [Qwen](https://qwen.aliyun.com/) | ✅ | — |
| [OpenCode](https://opencode.ai/) | ✅ | — |
| [Kimi Code](https://kimi-code.moonshot.cn/) | — | ✅ |
| [Pi](https://pi.ai/) | — | ✅ |

其他兼容 MCP 的智能体可以手动通过 `linux-x11-harness mcp` 连接。

## 手动命令

日常使用无需执行以下命令。它们主要用于调试或不通过智能体直接运行：

```bash
linux-x11-harness serve   # 启动守护进程
linux-x11-harness mcp     # 运行 MCP stdio 代理
linux-x11-harness status  # 查看守护进程状态
linux-x11-harness stop    # 停止守护进程
```

## 文档

- [使用指南](docs/usage.md) — 显示预览、socket 与多智能体隔离。
- [智能体技能](skills/linux-x11-harness/SKILL.md) — 给智能体的快速参考。
- [架构说明](docs/ARCHITECTURE.md) — crate 结构和设计说明。

## 许可证

MIT
