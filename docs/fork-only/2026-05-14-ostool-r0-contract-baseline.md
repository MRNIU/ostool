# 2026-05-14 OSTool R0 contract baseline

本文是 `2026-05-14-ostool-architecture-refactor-plan.md` 的 R0 阶段细化文档。它只记录现有
代码行为、真实命令验证结果和后续阶段需要保护的 contract，不引入生产代码改动。

## 目标

- 在 R1 拆 `Tool` 文件前，区分硬兼容、可废弃、内部实现和文档漂移。
- 用当前 CI 命令建立 Docker 验证基线，避免只靠静态阅读做结论。
- 记录真实缺口：哪些已有测试覆盖，哪些只能在后续阶段先抽 seam 再补 contract test。

## Docker 复核记录

复核环境：

- Docker image：`rust:1.90-bookworm`
- 平台：`--platform linux/amd64`
- Rust：`rustc 1.90.0 (1159e78c4 2025-09-14)`，host `x86_64-unknown-linux-gnu`
- Node：`v24.15.0`
- pnpm：`10.33.0`
- 容器内安装：`rustup component add rustfmt clippy`，`apt install qemu-system-aarch64 -y`，
  `apt install u-boot-tools -y`，`apt install libudev-dev -y`

已跑过的真实命令：

```bash
cargo fmt --all -- --check
cargo clippy --target x86_64-unknown-linux-gnu --all-features
cargo build --target x86_64-unknown-linux-gnu --all-features
cargo test --target x86_64-unknown-linux-gnu -- --nocapture
```

结果和环境结论：

- `cargo fmt --all -- --check` 通过。
- `cargo clippy --target x86_64-unknown-linux-gnu --all-features` 通过。
- `cargo build --target x86_64-unknown-linux-gnu --all-features` 通过。
- `cargo test --target x86_64-unknown-linux-gnu -- --nocapture` 在最小化 apt 环境中出现过两个环境型失败：
  `fitimage` 依赖 `mkimage` 调用 `dtc`，QEMU byte-stream 测试依赖 QEMU ROM/firmware。二者不是代码
  contract 失败，而是 Docker 命令人为使用 `--no-install-recommends` 后与 CI 环境不等价。
- 按 CI 风格使用普通 `apt install ... -y` 后，`ostool/tests/qemu_byte_stream.rs` 的 3 个 QEMU
  byte-stream 测试通过。
- 按 CI 风格继续跑全量 `cargo test --target x86_64-unknown-linux-gnu -- --nocapture` 时，FIT、jkconfig、
  ostool、主 CLI parser、public API trybuild、QEMU byte-stream 和 ostool-server 单元测试均已通过；
  随后测试卡在
  `ostool-server/tests/session_ws_lifecycle.rs::abrupt_ws_drop_powers_off_and_releases_session`，数分钟无输出，
  本次手动停止容器。因此当前不能宣称全仓库 test green。

后续复现注意：

- `ostool-server/build.rs` 会调用 `pnpm install --frozen-lockfile` 构建 webui。Docker 里只装 Rust
  不够，必须补齐 Node 24 和 pnpm 10.33.0。
- 不要用 `apt --no-install-recommends` 简化 QEMU/U-Boot 安装来判断 CI 等价性；CI 的普通
  `apt install qemu-system-aarch64 -y` 和 `apt install u-boot-tools -y` 会带入必要的推荐包。
- 全量 test 的当前未闭合项是 `ostool-server` WebSocket lifecycle 集成测试卡住。它不推翻 R0 对
  `ostool` build/run/board contract 的静态结论，但会阻止把“当前 CI 命令全通过”作为重构前基线。
  R1 之前至少要在 CI 或带外 Docker 里确认该测试是否稳定，或把它拆成独立修复项。

## Contract 分级清单

| 区域 | 等级 | 当前代码真相 | 已有覆盖 | R0 缺口和后续处理 |
|---|---|---|---|---|
| CLI 入口 | 硬兼容 | 主 CLI 入口是 `ostool build`、`ostool run qemu`、`ostool run uboot`、`ostool board ...`、`ostool menuconfig`，全局 manifest 参数是 `--manifest` | `ostool/src/main.rs` 目前主要覆盖 board 子命令解析 | R0/R1 前补或至少记录 `build`、`run qemu`、`run uboot`、`--manifest`、`--package`、`--bin` 的 parser contract |
| `cargo-osrun` | 硬兼容 | 独立 binary，解析 Cargo runner 传入的 `program`、`elf`、`--to-bin`、`--config`、`--no-run`、`--debug`、`--dtb-dump`，默认跑 QEMU，`uboot` 子命令跑 U-Boot | 当前没有同等 parser contract test | 不能把它当成主 CLI 的内部细节；R0/R6 需要保留入口语义 |
| Cargo build config | 硬兼容 | `system.Cargo` 支持 `package`、`bin`、`target`、`features`、`log`、`env`、`extra_config`、`profile`、`args`、`pre_build_cmds`、`post_build_cmds`、`to_bin` | 部分单元测试和集成命令覆盖 | R1 只移动代码；R2 才允许重整 build lifecycle |
| Cargo executable artifact 选择 | 硬兼容 | 通过 Cargo JSON message 选择 executable artifact；显式 `bin` 优先，其次 package 同名 bin、`default-run`、单一 bin，多 bin 无法判断时报错 | 行为在 `cargo_builder.rs` 私有函数里，缺少直接 contract test | R0 或 R2 第一笔补测试；这是 PR-03 debug artifacts 的前置边界 |
| Cargo profile/log-level feature | 硬兼容 | `profile` 未配置时，`ToolConfig.debug` 决定 Debug/Release；`system.Cargo.log` 会按 effective profile 生成 `log/max_level_*` 或 `log/release_max_level_*` feature | 目前缺针对 effective profile 的测试 | R2 改 build plan 前必须补 contract |
| someboot 自动参数 | 已知缺陷/内部实现 | build config 准备阶段和 Cargo command 构造阶段都有注入入口，存在重复追加风险 | 没有独立 contract | R0 记录，R1 保持现状，R2 收敛到唯一注入点 |
| Custom build: `ostool build` | 硬兼容 | `build_with_config()` 遇到 `system.Custom` 只执行 `build_cmd` | CI build 命令覆盖编译，不覆盖具体 custom 流程 | 不要把 run/board 的 runtime artifact 行为错误并入 `ostool build` |
| Custom build: run/board runtime | 硬兼容 | `prepare_runtime_artifacts()` 遇到 `system.Custom` 会执行 `build_cmd`，再按 `elf_path` 和 `to_bin` 准备 runtime artifact | 缺最小测试 | R2/R4 前保持行为；如果未来避免重复 build，需要新增可见迁移路径 |
| Artifact state | 硬兼容到可废弃过渡 | `OutputArtifacts` 当前只有 `elf`、`bin`、`cargo_artifact_dir`、`runtime_artifact_dir`，没有区分 Cargo 原始产物、runner runtime 产物和未来 debug artifacts | public API trybuild 只覆盖部分构造方式 | R2 新增清晰状态模型时，旧字段先映射或 deprecated |
| QEMU runner | 硬兼容 | `config.to_bin` 为真时生成 `.bin`；默认 machine 由 arch 推导；`--dtb-dump` 写 `target/qemu.dtb`；非 UEFI 默认 `-kernel`，优先 `bin` 后 `elf`；UEFI pflash/ESP 路径关闭 kernel loader | 有 QEMU byte-stream 集成测试，但没有 command-plan contract | 当前命令组装和 spawn 耦合，R0 不适合硬写 plan test；R5 第一笔先抽 command plan seam |
| U-Boot runner/FIT | 硬兼容到可扩展 | 当前 U-Boot runner 会准备 runtime `.bin` 并生成 `image.fit`；默认 FIT kernel component 偏 Linux/raw-bin 语义 | 有现有单元/集成覆盖，但 SimpleKernel 需要的新 FIT 语义未覆盖 | R3 抽 FIT 服务；旧默认继续映射，新配置走新增接口 |
| Board run/session | 硬兼容 | `.board.toml`、CLI override、全局 board config 共同解析；server/port 优先级是 CLI > project config > global config；远端 session 有 acquire/retry、heartbeat、release；当前只支持 `uboot` boot mode | 有部分 board 配置/session 单元测试 | 重构不能弱化 release、heartbeat 和 no-available-board retry；真实 board server 仍需手动验证 |
| 变量替换 | 硬兼容 + 文档漂移 | 代码支持 `${workspace}`、`${workspaceFolder}`、`${package}`、`${tmpDir}`、`${env:VAR}`；环境变量不存在时替换为空字符串 | `tool.rs` 已有 workspace/tmpDir/package 测试 | README 的 `${env:VAR:-default}` 是文档漂移，不是当前行为；应单独修文档或新增兼容功能 |
| menuconfig hooks | 硬兼容 | build config 使用 package/features/target hooks；QEMU/U-Boot 配置通过 schema 写回 | 相关 helper 有单元测试 | R1 拆文件时保持 hooks 行为，不顺手重写交互逻辑 |
| 输出匹配和 timeout | 硬兼容 | success/fail regex 编译后运行；默认 fail pattern 包含 panic/kernel panic；fail 优先；匹配后 drain；`timeout = None` 或 `0` 表示禁用 | `qemu_byte_stream` 用真实 QEMU 验证 byte-stream matcher 的换行前匹配和 fail 优先 | R6 前保持返回语义；R6 再抽 execution result |
| public API | 软兼容/可废弃 | `src/lib.rs` 公开 `build`、`board`、`ctx`、`logger`、`menuconfig`、`run`、`sterm`、`utils`，并 re-export `Tool`、`ToolConfig`、`ManifestContext`、`resolve_manifest_context` | `ostool/tests/public_api.rs` + trybuild 覆盖部分工具构造和 runner config | public surface 比主计划示例更宽；改名或改语义前先 deprecated |
| README/文档 | 文档漂移 | README 中仍有与当前代码不一致的变量替换语法和 CLI 叙述 | 无 | R0 只标记漂移；不要把旧 README 文本当当前代码 contract |

## 查漏补缺结论

1. R0 不能只写“现有行为保持”。每一项要带代码真相、测试证据和缺口，否则后续 PR review 时无法判断
   行为变化是 bug、迁移还是计划内内部调整。
2. `Custom build` 原计划写法过粗，必须拆成 `ostool build` 和 run/board runtime 两条路径。
3. QEMU `-kernel`、UEFI、`dtb_dump` 的 contract 目前没有纯函数入口。不要在 R0 伪造字符串扫描测试；
   R5 第一笔先抽 command plan，再补可维护测试。
4. `cargo-osrun` 是独立用户入口，不应被主 CLI parser 测试覆盖情况掩盖。
5. public API 的真实边界比 `Tool` 相关 re-export 更大。R1 拆文件可以调整内部模块，但不能无迁移地
   缩窄 `src/lib.rs` 当前公开模块。
6. Docker 复核显示 CI 等价环境必须包含 Node/pnpm 和 QEMU/U-Boot 推荐依赖。缺这些依赖时的失败要记为
   环境问题，不能当作架构问题。
7. 全量 `cargo test` 当前不是 clean baseline：`ostool-server` 的 abrupt WebSocket drop lifecycle
   集成测试会卡住，需要作为测试稳定性 follow-up，而不是在 R0 文档里隐藏。

## R0 最小补测建议

优先级从高到低：

1. CLI parser：补 `build --config --package --bin`、`run qemu --config --qemu-config --debug --dtb-dump`、
   `run uboot --config --uboot-config`、全局 `--manifest`。
2. `cargo-osrun` parser：补默认 QEMU、`uboot` 子命令、`--to-bin`、`--no-run`、`--build-dir`、`--bin-dir`。
3. Cargo artifact 选择：补显式 `bin`、package 同名 bin、`default-run`、单一 bin、多 bin 歧义报错。
4. Cargo profile/log：补 debug/release effective profile 与 `log` feature 组合。
5. Custom runtime：补 `ostool build` 只执行 `build_cmd`，run/board runtime 路径执行 `build_cmd + elf/to_bin`
   准备的差异。
6. Board session：保留 no-available-board retry、board-type-not-found 立即失败、release 调用、heartbeat 停止。

QEMU command plan、U-Boot FIT 生成和 execution result 建议放到 R3/R5/R6 的第一步抽 seam 后再补，不在
R0 用脆弱的源文本扫描代替行为测试。
