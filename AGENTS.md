# AGENTS.md - ostool

## 适用范围

本文件适用于整个仓库。子目录中的 `AGENTS.md` 只对对应目录生效，并覆盖或补充本文件规则。

## 项目结构

- `ostool/`: 主 CLI/库，覆盖 OS 构建、`menuconfig`、QEMU、U-Boot、TFTP、串口、
  board-client 和 `cargo-osrun` 流程。
- `ostool-server/`: 开发板管理服务器，包含 API、串口会话、TFTP 文件、电源管理和
  面向 systemd 的部署脚本。
- `ostool-server/webui/`: `ostool-server` 嵌入的 Vue/Vite/pnpm 前端。
- `jkconfig/`: 基于 Ratatui 的 JSON Schema 配置编辑器库，并提供可选 web 功能。
- `fitimage/`: 用于构建 U-Boot 兼容 FIT 镜像的库。
- `uboot-shell/`: 异步 U-Boot shell 与 YMODEM 通信库。
- `.github/workflows/check.yaml`: 当前格式化、clippy、构建和测试命令的 CI 来源。

## 依赖与工具

- 复现检查时，优先使用本仓库或 CI 声明的工具链和依赖版本。
- 不要为了通过检查临时引入仓库未声明的工具、依赖或配置；需要新增或替换工具链、
  包管理器、安装脚本等时，应作为独立变更并说明依据。
- 当前环境缺少必要工具时，说明缺失工具和未能运行的检查。

## Git 与提交

- 本 checkout 是从上游社区仓库 fork 出来的工作副本。默认把 `origin` 视为个人 fork，
  把 `upstream` 视为上游社区仓库；执行 push、开 PR 或删除远端分支前先确认目标远端。
- fork 侧的本地维护文档、AI 协作规则和迁移计划可以正常保留在本地 checkout 或个人 fork 中。
  准备向上游提交 PR 时，检查 `git status`、`git diff upstream/main...HEAD` 和 staged diff，
  把 `AGENTS.md`、计划文档、临时记录或其他无关本地修改从上游 PR 中剔除即可。
- `docs/fork-only/` 只服务于本 checkout 或个人 fork 的迁移、重构、审计和临时规划，不提交到
  上游仓库，也不放进面向上游的 PR。
- 在当前仓库中新生成的计划文档、阶段记录、审计记录、临时分析、AI 协作资料等 fork 侧文件，
  默认都放入 `docs/fork-only/`；只有明确面向上游或用户文档的内容才放到公开文档路径。
- 准备向上游提交 PR 前，必须检查 `git diff --name-only upstream/main...HEAD`，确认 diff 中没有
  `AGENTS.md`、子目录 `AGENTS.md`、`docs/fork-only/` 或其它 fork-only 资料。
- 向上游提交 PR 时遵循 README 的社区贡献流程：从 fork 创建描述任务的 `feature/...`
  分支，保持一个 PR 只解决一个清晰主题，并在 PR 说明中写清变更原因、影响范围和实际
  运行过的验证命令。无法运行的检查要如实说明。
- 除非用户明确要求留在当前分支，否则仓库改动应在功能分支上完成。
- 遵循近期提交风格，使用 Conventional Commits，例如 `fix(ostool): ...`、
  `chore(ostool-server): ...`、`docs: ...` 或 `refactor(jkconfig): ...`。
- 不要把无关改动混入同一个提交。只暂存属于当前任务的文件。

## 验证

- 仓库级 Rust 改动应在可行时复现当前 CI target matrix 中的格式化、clippy、构建和
  测试命令。当前 `.github/workflows/check.yaml` 使用
  `x86_64-unknown-linux-gnu`。
- 局部改动优先运行覆盖被修改区域的最小 package 或 web UI 检查，并明确说明实际运行
  或未运行的命令。
- CI 会安装 QEMU、U-Boot tools、libudev、Node.js 24 和 pnpm 10.33.0。把这些视为
  CI 验证环境证据，不要据此要求贡献者准备同一批工具。

## 文档

- 用户可见的 CLI、服务器 API、配置格式、安装路径或工作流发生变化时，同步更新相关
  README 或局部文档。
- 根 README 同时有中文和英文版本。修改共享的用户文档时，保持两个版本一致。
- changelog 按 package 拆分。只有发布或用户明确要求时才更新 changelog。

## Rust 约定

- 保持各 package 已采用的 edition 和公开 API 风格。
- 默认保持既有 CLI、配置格式和 public API 兼容；如果旧接口阻碍更清晰的长期模型，可以新增
  替代接口，并把旧接口标为 deprecated。Rust API 使用 `#[deprecated]`，CLI/config 通过 README、
  配置示例、解析提示或 warning 说明迁移路径和未来移除计划。
- 配置数据优先使用已有 `serde`、`schemars`、TOML 和 JSON 类型进行结构化解析或序列化，
  不要手写脆弱的字符串处理。
- 串口、TFTP、QEMU、U-Boot 和开发板电源相关改动属于操作敏感路径。副作用必须明确，
  并通过测试或记录过的手动验证覆盖。
