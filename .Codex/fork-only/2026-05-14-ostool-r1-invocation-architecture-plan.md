# R1 Invocation 架构实施计划

> **给智能协作执行者的说明：** 逐项执行本文档时使用 `superpowers:executing-plans`。只有用户明确要求时才使用 subagents；如果使用，按独立文件所有权拆分任务，阻塞性的集成和最终收口留在本地主线完成。

## 目标

R1 的目标是移除 `Tool` 作为中心业务对象的架构，把一次 OSTool 调用拆成更清晰的 invocation 模型，同时保持 CLI、配置格式和运行时行为不变。

### 2026-05-23 状态更新

上游 PR #108 已合入，merge commit 为 `bed5315 refactor(tool): 拆分调用、项目与进程上下文 (#108)`。该 PR 采用
**Upstream-friendly mode**，不是本文最初设想的 fork-side 一次性 API reset。#108 之后，本地
R1d 已继续完成 build outcome seam，但仍保持 CLI、配置格式和运行时行为不变。

### 2026-05-29 代码现状复核

上游 PR #111 已合入，commit 为 `9f61aac refactor(tool): 拆分构建与运行产物边界 (#111)`。
之后上游 PR #114 已合入，commit 为 `b06de09 refactor(tool): 收敛构建配置与运行产物边界 (#114)`。
之后上游 PR #115 已合入，commit 为 `09a1297 refactor(tool): 接入 invocation 构建状态 (#115)`。
本次复核以当前 `main` 的实际代码路径为准，而不是只看 PR 标题或旧计划清单。

已落地的边界：

- 新增 `InvocationOptions` / `Invocation`，由 CLI 和 `cargo-osrun` 先解析 invocation，再创建兼容 `Tool` 门面。
- 新增 `ProjectLayout`、Cargo metadata helper、`VariableScope` 和 `ProcessContext`。
- `project` / `process` 保持 crate-internal，`invocation` 作为当前最小公开入口。
- build hooks、QEMU、U-Boot、board config 和 shell command 构造已改用 `VariableScope` / `ProcessContext`。
- `OutputArtifacts` 已迁入 `artifact/state.rs`，`ctx.rs` 通过 re-export 保留兼容 API。
- `cargo_builder.rs` 已拆为 `cargo_pipeline.rs` / `artifact_selector.rs`：
  `CargoBuildPipeline::execute()` 返回 `CargoBuildOutcome`，artifact selection 独立测试，
  orchestration 层消费 resolved artifact，并通过 `InvocationState` / legacy bridge 同步 runtime state。
- 新增 `ostool/src/artifact/runtime.rs`：
  `prepare_runtime_artifacts()` 统一处理 ELF canonicalization、arch detection、stripped runtime `.elf`
  和 optional runtime `.bin`；`PreparedRuntimeArtifacts` 再由兼容桥接写入 `InvocationState` 并同步旧
  `Tool.ctx.artifacts`。
- Cargo build / run path 已变成：`CargoBuildPipeline::execute()` 返回 `CargoBuildOutcome`，
  `build/mod.rs` 的 orchestration 层消费 outcome，再调用 runtime artifact helper；`CargoBuildPipeline`
  不再持有 `&mut Tool`，只接收显式 `CargoBuildInput`。
- 新增 `build/config_loader.rs` 和 `build/config_hooks.rs`；`Tool::prepare_build_config()` 已委托 loader，
  `.build.toml` 菜单 hooks 已迁出 `Tool`。
- #115 新增 `InvocationState` / `ActiveBuildContext` 生产接线：build config 激活、Cargo/custom build scope、
  prepared runtime artifact 和 arch 先写入 `InvocationState`，再同步到旧 `AppContext` 兼容层。
- 上游 #106 的 `disable_someboot_build_config` 语义保留。

仍未完成的 R1 目标：

- `Tool`、`ToolConfig`、`ManifestContext`、`ctx::OutputArtifacts` re-export 仍是兼容 API，不能写成已移除。
- QEMU、U-Boot、board 仍有跨模块 `impl Tool`，runner/board entrypoint 尚未完全改成显式输入。
- QEMU、U-Boot、board runner 仍通过 `Tool` 兼容门面取 runner artifact；生产读取优先走
  `InvocationState.artifacts`，但 runner/board entrypoint 尚未直接接收 `PreparedRuntimeArtifacts`
  或其它显式 artifact state。
- debug artifact registry 尚不存在；artifact lifecycle、someboot 注入责任收敛、debug artifact pipeline
  仍属于 R2 及后续工作。

后续默认继续走 **Upstream-friendly incremental mode**：每个切片都应保持 CLI/config/runtime 行为不变，避免一次性 public API break。只有用户明确要求 fork-side internal API reset 时，才按本文的最终形态删除 `Tool` / `ctx`。

核心思路：

- `ProjectLayout` 保存不可变的项目路径事实。
- `InvocationOptions` 保存一次命令调用的静态选项。
- `InvocationState` 保存一次命令执行期间产生的可变状态。
- `Invocation` 只组合 layout/options/state，不承载 build/run/menuconfig 业务行为。
- build、artifact、runner、board、menuconfig 通过明确的模块函数或确实有状态的结构体协作。
- build pipeline 返回事实结果，不直接改全局 invocation state。
- runtime artifact preparation 独立于 Cargo artifact 选择，为后续 debug artifact pipeline 留出边界。
- R1 采用保守切片执行：允许 `Tool` 在中间阶段暂时作为兼容门面存在，但每个切片都必须让它变薄，最终删除。

技术栈保持现状：Rust、Clap、Tokio、Cargo metadata JSON messages、`object`、`jkconfig`，以及现有 OSTool build/run/board 模块。

---

## 1. 范围和兼容性定位

R1 是一次较大的内部架构重构。以下用户可见 contract 必须保持不变：

- `ostool build`
- `ostool run qemu`
- `ostool run uboot`
- `ostool board run`
- `ostool menuconfig`
- `cargo-osrun`
- `.build.toml`、`.qemu.toml`、`.uboot.toml`、`.board.toml`
- 当前变量替换语义：`${workspace}`、`${workspaceFolder}`、`${package}`、`${tmpDir}`、`${env:VAR}`
- 当前 runtime `.elf` / `.bin` 准备语义，包括 `to_bin`
- 当前 QEMU、U-Boot、board、serial、TFTP 和电源管理副作用

R1 可以破坏围绕 `Tool` 的 Rust library API：

- `Tool`
- `ToolConfig`
- `ManifestContext`
- `AppContext`
- `OutputArtifacts`
- `ostool::ctx`

这个破坏只在 fork-side 架构重构模式下成立。如果后续准备提交上游 PR，必须在开工前选择兼容模式。

### 1.1 兼容模式开关

R1 开工前必须明确采用哪一种模式：

| 模式 | 含义 | 执行要求 |
|---|---|---|
| Fork-side mode | 本 fork 内部架构重构，`Tool` 相关 Rust API 可一次性移除或改名 | CLI/config/runtime 行为硬兼容；PR 或提交说明写清这是 internal API reset |
| Upstream-friendly mode | 计划直接面向上游 review | 保留 deprecated wrapper/re-export，或者先确认上游接受 breaking Rust API |

2026-05-23 起，如果没有新的用户指令，当前文档默认按 **Upstream-friendly incremental mode** 执行。也就是说，
后续切片优先继续削薄 `Tool`，但不默认删除或破坏已公开 Rust API。

R1 明确不做：

- 不新增 `debug_artifacts` 配置。
- 不调用 `rust-objdump`、`rust-readobj` 或 `rust-nm`。
- 不改变 `to_bin` 语义。
- 不改变 Cargo executable artifact 选择规则。
- 不改变 someboot 自动参数语义；重复注入风险留到 R2。
- 不改变 QEMU、U-Boot、board、serial、TFTP 或电源管理副作用。
- 不把 generic `check`、`clippy`、`fmt`、`deny` 或 host-test 质量门禁塞进 OSTool。

## 2. 防代码膨胀和过度设计约束

`ostool/src/tool.rs` 当前约 1300 行。这个体量本身不是必须重构的理由。R1 的理由是 `Tool` 被跨模块 `impl Tool` 当成万能对象，导致 build、run、artifact、config、variable 都能互相触碰状态。

因此 R1 不追求文件数量增加，也不追求把一个文件拆成很多层。R1 的成功标准是净复杂度下降：

- 共享可变状态减少。
- 跨模块 `impl Tool` 消失。
- build outcome、runtime artifact、process context 等关键 seam 可测试。
- 调用方拿到的是窄输入和返回值，而不是万能对象。
- 中间阶段允许 `Tool` 作为兼容门面，但只能调用新 helper，不能新增业务逻辑。

### 2.1 文件和对象预算

执行 R1 时必须遵守以下预算：

- 不预创建完整目录树。只在当前切片需要时创建文件。
- 只有一个函数的 helper，优先留在现有模块或单个 `mod.rs` 中；等职责变成两个以上再拆文件。
- 无状态逻辑默认使用 module-level function，不创建 `*Service`、`*Factory`、`*Runner` 空壳对象。
- 只有持有真实状态、资源、lifetime boundary 或测试 seam 时，才引入 struct。
- 只有存在多个实现或明确 mock seam 时，才引入 trait。
- R1 相关生产代码如果净增超过约 20%-30%，必须在 review 中说明换来的真实收益；否则优先删减抽象。

推荐形态：

```rust
artifact_selector::select_executable_artifact(...)
project::variables::replace_string(scope, input)
process::run_shell_command(ctx, cmd)
```

避免形态：

```rust
ArtifactSelector::new().select(...)
VariableExpander::new().replace(...)
ShellCommandRunner::new().run(...)
```

除非这些对象确实持有状态或作为测试 seam。

### 2.2 保守切片不降低完整目标

“保守切片”不是缩小 R1 目标，而是让每一步都能编译、验证和回滚。完整目标仍然是：

- `Tool` 不再是核心模型。
- 跨模块 `impl Tool` 消失。
- `Invocation` 不成为新中心对象。
- Cargo executable resolution、runtime artifact preparation、runner execution 通过显式输入协作。

保守切片的执行方式是：

1. 先补行为护栏。
2. 再抽纯事实和纯函数。
3. 再抽返回值 seam。
4. 再迁移调用方。
5. 最后删除 `Tool` 和 `ctx`。

中间状态允许 `Tool` 暂时存在，但每个切片都必须满足：

- `Tool` 的业务职责减少。
- 不新增新的 `impl Tool` 业务方法。
- 新代码可以被旧 `Tool` 门面调用，但最终调用方向应从 `Tool -> helper` 迁移到 `Invocation/orchestration -> helper`。

## 3. 为什么必须移除 `Tool`

`Tool` 当前把多个概念压成一个可变对象：

- project discovery：manifest path、manifest dir、workspace dir。
- invocation options：build dir、bin dir、debug mode。
- invocation state：arch、build config path、build config、artifacts。
- command construction 和 shell execution。
- variable expansion。
- build config loading 和 menu hooks。
- runtime artifact preparation。
- 通过 Cargo build pipeline 编排 Cargo build。
- 通过跨模块 `impl Tool` 提供 QEMU、U-Boot、board、menuconfig 门面。

直接问题不是 `tool.rs` 文件长，而是 `Tool` 太方便传递，任何模块都能继续给它挂行为。R1 要消除这个中心吸力，而不是把 `tool.rs` 机械拆成多个继续 `impl Tool` 的小文件。

## 4. 目标概念

### 4.1 `ProjectLayout`

表示当前操作项目的不可变路径事实。

```rust
pub(crate) struct ProjectLayout {
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    workspace_dir: PathBuf,
}
```

职责：

- 从显式路径或当前目录解析 Cargo manifest。
- 保存 canonical package manifest path。
- 保存 package manifest directory。
- 保存 Cargo workspace root。

非职责：

- 不保存 build config。
- 不保存 arch。
- 不保存 artifacts。
- 不执行命令。

### 4.2 `InvocationOptions`

表示一次 OSTool 调用的静态选项。

```rust
pub(crate) struct InvocationOptions {
    manifest: Option<PathBuf>,
    build_dir: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
    debug: bool,
}
```

`manifest` 只用于 layout resolution。`build_dir`、`bin_dir` 和 `debug` 是一次 invocation 内稳定的默认值。

### 4.3 `InvocationState`

表示一次 invocation 执行过程中产生的可变状态。

```rust
pub(crate) struct InvocationState {
    arch: Option<object::Architecture>,
    active_build: Option<ActiveBuildContext>,
    artifacts: OutputArtifacts,
}
```

`InvocationState` 替代 `AppContext`。名字刻意使用 `State`，避免后续把任何依赖都塞进泛化的 `Context`。

要求：

- 字段保持 private。
- 只能通过小方法更新，例如 `set_arch`、`set_active_build`、`set_cargo_artifact`、`set_runtime_artifact`。
- service 优先接收自己需要的窄输入，不应因为方便而借用或克隆整个 `Invocation`。
- build pipeline 不直接写 `InvocationState`；它返回 outcome，由 orchestration 层更新 state。

### 4.4 `Invocation`

表示一次命令执行 session。

```rust
pub(crate) struct Invocation {
    layout: ProjectLayout,
    options: InvocationOptions,
    state: InvocationState,
}
```

职责：

- 从 `InvocationOptions` 创建 `ProjectLayout`。
- 拥有 `InvocationState`。
- 提供少量访问器，例如 `layout()`、`options()`、`state()`、`state_mut()`、`build_dir()`、`bin_dir()`。

非职责：

- 不负责 Cargo build lifecycle。
- 不负责 runtime artifact preparation。
- 不负责 QEMU/U-Boot/board execution。
- 不负责 config loading。
- 不负责 variable expansion 逻辑。

`Invocation` 不能变成新版 `Tool`。业务行为不得通过跨模块 `impl Invocation` 扩展。

### 4.5 `ActiveBuildContext`、`ActiveCargoBuild` 和 `VariableScope`

表示 CLI override 已经应用后的最终 build 状态。

```rust
pub(crate) enum ActiveBuildContext {
    Cargo(ActiveCargoBuild),
    Custom(ActiveCustomBuild),
}

pub(crate) struct ActiveCargoBuild {
    config: Cargo,
    config_path: Option<PathBuf>,
    package_dir: PathBuf,
    variable_scope: VariableScope,
}

pub(crate) struct ActiveCustomBuild {
    config: Custom,
    config_path: Option<PathBuf>,
    variable_scope: VariableScope,
}

pub(crate) struct VariableScope {
    workspace_dir: PathBuf,
    package_dir: PathBuf,
    tmp_dir: PathBuf,
}
```

职责：

- 保存 CLI override 后的最终 build config。
- 保存 build config path，用于 relative `extra_config` 和 shell hooks。
- 为 `${package}` 提供单一真相源。
- 避免从某个过期的 `BuildConfig` 副本重新推导 package dir。

关键规则：

- `--package` / `--bin` override 必须在创建 `ActiveCargoBuild` 前应用。
- 创建 `ActiveCargoBuild` 后，不再修改其中的 `Cargo` 语义字段。
- variable expansion 只读 `VariableScope`。
- 没有 active build 时，使用从 `ProjectLayout` 派生的 default `VariableScope`。

### 4.6 `CargoBuildOutcome`

Cargo build pipeline 返回构建事实，不直接修改 invocation state。

```rust
pub(crate) struct CargoBuildOutcome {
    executable: ResolvedCargoArtifact,
}

pub(crate) struct ResolvedCargoArtifact {
    elf_path: PathBuf,
    cargo_artifact_dir: PathBuf,
}
```

职责：

- 表示 Cargo JSON message 中解析出的 executable artifact。
- 保持 artifact selection 规则稳定。
- 给 runtime preparation 提供输入。

非职责：

- 不保存 runner runtime `.elf` / `.bin`。
- 不保存未来 debug artifacts。
- 不决定 QEMU/U-Boot/board 行为。

### 4.7 `ProcessContext`

变量替换、命令构造和 shell hook 需要明确输入，而不是依赖 `Tool`。

```rust
pub(crate) struct ProcessContext<'a> {
    workdir: &'a Path,
    workspace_dir: &'a Path,
    variables: &'a VariableScope,
    kernel_elf: Option<&'a Path>,
}
```

要求：

- `command()` 使用 `workdir` 作为工作目录。
- `WORKSPACE_FOLDER` env 行为保持当前语义。
- 参数和 env 值通过 `VariableScope` 替换。
- shell hook 在已有 runtime ELF 时继续注入 `KERNEL_ELF`。
- `KERNEL_ELF` 不只属于 Cargo post-build；当前所有 `shell_run_cmd()` 在 artifact 存在时都会注入它，R1 不应缩小这个语义。

### 4.8 Rust API 形态规则

R1 应使用 Rust-native 模块边界，不要为了“看起来抽象”到处创建 nominal service object。

- 没有 owned state、resource、cache 或 lifetime boundary 时，优先使用 module-level functions。
- `CargoBuildPipeline`、`RuntimeArtifactPreparer`、`QemuRunner` 等结构体只有在确实持有执行状态或依赖时才创建。
- 只有存在多个实现或明确测试 seam 时才新增 trait。
- 避免结构体长期持有很多 `&mut` 引用；优先显式函数参数和短 borrow scope。
- 命名描述领域操作，不描述空泛模式。例如 `artifact_selector::select_executable_artifact` 优于无状态的 `ArtifactSelector` 对象。

## 5. 目标目录结构

以下目录结构是完整 R1 的目标形态，不是 #108 合入后的当前形态。#108 已创建 `invocation.rs`、
`project/*` 和 `process/mod.rs`；#108 之后已把 `CargoBuilder` 拆为 `CargoBuildPipeline` /
`artifact_selector`，但为了上游兼容仍保留 `tool.rs`、`ctx.rs` 和跨模块 `impl Tool`。后续切片
修改本表时，必须区分“已合入上游的中间状态”和“完整 R1 终态”。

```text
ostool/src/
  lib.rs                         [modify] 更新导出；移除 `tool` 作为中心模块
  main.rs                        [modify] 创建 `Invocation`，调用显式服务
  invocation.rs                  [create] Invocation, InvocationOptions, InvocationState

  ctx.rs                         [delete] AppContext 角色迁移到 InvocationState；OutputArtifacts 迁移到 artifact/state.rs
  tool.rs                        [delete] 移除 Tool, ToolConfig, ManifestContext

  project/                       [create] project discovery 和项目本地 helper
    mod.rs                       [create]
    layout.rs                    [create] ProjectLayout, resolve_project_layout()
    metadata.rs                  [create] cargo metadata、package lookup、package manifest dir
    variables.rs                 [create] VariableScope 和变量替换 helper

  process/                       [create] process context、command construction、shell execution
    mod.rs                       [create]
    command.rs                   [create] command construction helpers
    shell.rs                     [create] shell command execution helpers

  artifact/                      [create] runtime artifact state 和 preparation
    mod.rs                       [create]
    state.rs                     [create] OutputArtifacts
    runtime.rs                   [create] RuntimeArtifactPreparer, PreparedRuntimeArtifacts

  build/
    mod.rs                       [modify] 暴露明确 build functions/types，不再 `impl Tool`
    config.rs                    [keep] R1 不改用户可见配置语义
    someboot.rs                  [keep] R1 不改 someboot 语义
    cargo_builder.rs             [delete or rename] 改为 cargo_pipeline.rs
    cargo_pipeline.rs            [create] CargoBuildPipeline / run_cargo_build()
    artifact_selector.rs         [create] Cargo JSON executable artifact selection
    config_loader.rs             [create] BuildConfigLoader
    config_hooks.rs              [create] jkconfig hooks for build config

  run/
    mod.rs                       [modify] runner entrypoints 不再依赖 Tool
    qemu.rs                      [modify] 显式 QEMU API；可保留有状态 QemuRunner
    uboot.rs                     [modify] 显式 U-Boot API；可保留有状态 UbootRunner
    tftp.rs                      [modify] 接收 concrete layout/artifact/process inputs
    output_matcher.rs            [keep]
    shell_init.rs                [keep]
    ovmf_prebuilt/               [keep]

  board/
    mod.rs                       [modify] 显式 BoardRunner 或 board functions
    config.rs                    [modify] 使用变量替换 helper，不依赖 Tool
    global_config.rs             [keep]
    client.rs                    [keep]
    session.rs                   [keep]
    serial_stream.rs             [keep]
    terminal.rs                  [keep]
    config_tui.rs                [keep]

  menuconfig.rs                  [modify] 使用 BuildConfigLoader/config hooks
  logger.rs                      [keep]
  sterm/mod.rs                   [keep]
  utils.rs                       [modify] 保留通用 helper；命令/变量相关逻辑迁出
  bin/cargo-osrun.rs             [modify] 创建 Invocation，调用 artifact/runner services
```

测试和文档：

```text
ostool/tests/public_api.rs                  [modify] 不再断言 Tool-centered API
ostool/tests/ui/pass_tool_configs.rs        [delete or rewrite] 只保留有意支持的新 API
ostool/tests/ui/fail_cargo_pipeline.rs      [modify] cargo pipeline privacy check
ostool/tests/ui/*.stderr                    [modify] 更新 trybuild expected output
ostool/tests/qemu_byte_stream.rs            [keep]

.Codex/fork-only/2026-05-14-ostool-architecture-refactor-plan.md [modify] 链接 R1 完成状态
```

## 6. 依赖方向

R1 应形成以下依赖方向：

```mermaid
graph TD
    CLI["main.rs / cargo-osrun"] --> Invocation["Invocation"]
    CLI --> Build["build services"]
    CLI --> Artifact["artifact services"]
    CLI --> Run["runner services"]
    CLI --> Board["board services"]
    CLI --> Menu["menuconfig"]

    Invocation --> Project["project layout"]
    Invocation --> ArtifactState["artifact state"]

    Build --> Project
    Build --> Process["process services"]
    Build --> ActiveBuild["ActiveBuildContext"]
    Build --> BuildOutcome["CargoBuildOutcome"]

    Artifact --> ArtifactState
    Artifact --> BuildOutcome
    Artifact --> Process

    Run --> Project
    Run --> Process
    Run --> ArtifactState

    Board --> Project
    Board --> Run
    Board --> ArtifactState

    Menu --> Build
    Menu --> Project
```

禁止依赖：

- 任何模块都不应新增带业务行为的 `impl Invocation`。
- service API 不应在窄输入足够时接收 `&Invocation` 或 `&mut Invocation`。
- `CargoBuildPipeline` 不应接收 `&mut InvocationState`。
- build pipeline 返回 `CargoBuildOutcome`，由 orchestration 层更新 `InvocationState`。
- runner 模块不依赖 build config loading。
- build config loading 不依赖 QEMU/U-Boot runner execution。
- variable expansion 不修改 `InvocationState`。
- command construction 不知道 build-system-specific 语义。

## 7. 实施计划

R1 按 R1a-R1i 保守切片执行。切片编号是执行顺序，不改变最终完整目标。

### 2026-05-29 当前切片状态

| 切片 | 当前状态 | 说明 |
|---|---|---|
| R1a contract tests | 已完成（#108 范围） | 已补 `ostool build`、`run qemu`、`run uboot`、`board run` 和 `cargo-osrun` parser 护栏；已补变量替换、missing env、selected package、process env/args、`KERNEL_ELF` shell hook、Cargo artifact selector、resolved artifact 写回 runtime state 等测试。原计划中的完整 public API reset 护栏不适用于 #108 的上游友好模式。 |
| R1b project/invocation/process seed | 已完成（#108/#115 范围） | `ProjectLayout`、metadata helper、`InvocationOptions` / `Invocation`、`VariableScope`、`ProcessContext` 已落地；CLI 与 `cargo-osrun` 已先创建 `Invocation`，再创建兼容 `Tool` 门面。`InvocationState` / `ActiveBuildContext` 已在 #115 中接入 build/runtime 生产路径。 |
| R1c variable/process functions | 已完成（#108 范围） | 变量替换、path expansion、command 构造和 shell hook 已从 `Tool` 逻辑抽到 project/process helper；build、QEMU、U-Boot、board config 等调用点已通过兼容 `Tool` 取得 `VariableScope` / `ProcessContext`。 |
| R1d resolved Cargo artifact seam | 已完成（#111 已合入） | `artifact_selector.rs` 已承载 Cargo JSON executable selection 并覆盖 explicit bin、package-name binary、`default-run`、single binary 和 ambiguity error；`CargoBuildPipeline::execute()` 返回 `CargoBuildOutcome`，且测试证明 pipeline 不直接写旧 `Tool.ctx.artifacts`。 |
| R1e runtime artifact preparer | 已完成当前上游友好目标（#111/#114/#115） | `artifact/runtime.rs`、`RuntimeArtifactOptions`、`PreparedRuntimeArtifacts`、`prepare_runtime_artifacts()` 和 `artifact/state.rs::OutputArtifacts` 已落地。Cargo outcome 和 custom ELF path 都通过 orchestration/compat bridge 准备 runtime artifact；`PreparedRuntimeArtifacts` 先写入 `InvocationState`，再同步旧 `Tool.ctx.artifacts`。`ctx.rs` re-export 保留为兼容 API。 |
| R1f build config loader/menu hooks | 已完成当前上游友好目标（#114/#115） | `build/config_loader.rs` 和 `build/config_hooks.rs` 已落地，`Tool::prepare_build_config()` 已委托 loader，menu hooks 已迁出 `Tool`。CLI override 后会创建 `ActiveBuildContext`；Cargo/custom build orchestration 通过 `InvocationState` 记录 active build 和 runtime artifacts。`Tool` 仍保留为 public compatibility facade。 |
| R1g runner/board entrypoints | 未完成 | QEMU、U-Boot、board 仍保留跨模块 `impl Tool`。 |
| R1h remove `Tool` / `ctx` | 未开始 | 上游友好模式下暂不默认执行；除非另起 fork-side reset 或上游明确接受 API break。 |
| R1i final verification/docs | 部分完成 | #108/#111/#114/#115 均已合入上游；#115 实现分支曾运行 CI 同形态命令：`cargo fmt --all -- --check`、`cargo clippy --target x86_64-unknown-linux-gnu --all-features`、`cargo build --target x86_64-unknown-linux-gnu --all-features`、`cargo test --target x86_64-unknown-linux-gnu -- --nocapture`。完整 R1 终态验证尚未发生。 |

下方 R1a-R1i 任务清单保留为完整 R1 终态 checklist。因为 #108 采用了更小的上游友好实现形态，
没有逐项机械勾选所有原始 checkbox；当前完成状态以上表和各任务的“实现校准”说明为准。

下一步建议不要继续把所有剩余项都塞进 R1e。更合适的是先按实际消费者拆成小切片：

- 若目标是继续 R1 cleanup：先收敛 runner/board entrypoints，让 QEMU、U-Boot、board 逐步接收
  prepared runtime artifact 或更窄的 artifact state，而不是继续依赖 `Tool` 兼容门面。
- 若目标是进入下一项用户价值：可以开始 PR-03 debug artifact pipeline，但必须把 debug artifact
  registry / object tools 边界作为 R2 的窄切片处理，避免重新扩大 R1。
- 精简或真正接线尚未消费的中间模型，避免 `InvocationState` 或 `ActiveBuildContext` 在没有消费者时回流成空壳抽象。
- 保持 `project` / `process` crate-internal，除非有清晰 public API 需求。
- 把 someboot 注入责任收敛留到 R2，不塞回 R1 follow-up。

### 任务 1 / R1a：先补 contract tests，再移动代码

**文件：**

- Modify: `ostool/src/main.rs`
- Modify: `ostool/src/bin/cargo-osrun.rs`
- Modify: `ostool/tests/public_api.rs`
- Modify or replace: `ostool/tests/ui/pass_tool_configs.rs`
- Add or move tests near: `project::variables`、`build::artifact_selector`、`artifact::runtime`、`process`

步骤：

- [x] 补 `ostool build --config --package --bin` parser tests。
- [x] 补 `ostool run qemu --config --qemu-config --debug --dtb-dump --package --bin` parser tests。
- [x] 补 `ostool run uboot --config --uboot-config --package --bin` parser tests。
- [x] 补 global `--manifest` parser tests。
- [x] 补 `cargo-osrun` parser tests：default QEMU、`uboot`、`--to-bin`、`--build-dir`、`--bin-dir`。
- [x] 补 Cargo artifact selector tests：显式 `bin`、package 同名 bin、`default-run`、single bin、多 bin ambiguity。
- [x] 补 variable replacement tests：`${workspace}`、`${workspaceFolder}`、`${package}`、`${tmpDir}`、`${env:VAR}`、missing env -> empty string。
- [x] 补 process context tests：workdir、`WORKSPACE_FOLDER`、arg/env replacement。
- [x] 补 shell context tests：runtime ELF 存在时 shell hook 注入 `KERNEL_ELF`。
- [x] 补 runtime artifact state tests：Cargo artifact path 和 resolved Cargo artifact 写回旧状态等价。
- [x] 上游友好模式下保留 `Tool` public API，未执行完整 public API reset。
- [x] #108 已运行并记录 `cargo test -p ostool -- --nocapture`。
- [x] Run parser/selector/variable/process/artifact 相关最小测试。

审查重点：

- 测试必须证明行为，不做 source grep。
- 不应无意创建新的 stable Rust API promise。
- selector 和 variable/process tests 是 R1 的行为护栏，不应推迟到 R2。

### 任务 2 / R1b：引入 Project、VariableScope 和兼容门面

**文件：**

- Create: `ostool/src/invocation.rs`
- Create: `ostool/src/project/mod.rs`
- Create: `ostool/src/project/layout.rs`
- Create: `ostool/src/project/metadata.rs`
- Create or keep local: `ostool/src/project/variables.rs`
- Create or keep local: `ostool/src/process/mod.rs`
- Modify: `ostool/src/lib.rs`
- Modify: `ostool/src/main.rs`
- Modify: `ostool/src/bin/cargo-osrun.rs`

步骤：

- [x] 把 manifest path resolution 移到 `project/layout.rs`。
- [x] 把 `ManifestContext` 的项目事实语义迁移到 `ProjectLayout`，并保留 `ManifestContext` 作为兼容类型。
- [x] 把 package metadata lookup 移到 `project/metadata.rs`。
- [x] 引入 `InvocationOptions` 和 `Invocation`；#108 未引入无生产消费者的 `InvocationState`。
- [x] 保持 `ProjectLayout`、`InvocationOptions`、`Invocation` 字段 private。
- [x] 引入 `VariableScope` 和 `ProcessContext`；`ActiveBuildContext` 系列未进入 #108 当前实现。
- [x] 先让现有 `Tool` 门面调用这些新 helper；本切片不要求删除 `Tool`。
- [x] 确认 `Tool` 没有新增业务职责，只是转发到新 helper。
- [x] 用 invocation constructor 替代 `init_tool()` 内部 manifest resolution。
- [x] 替换 `cargo-osrun` 里的 `resolve_manifest_context()` 用法。
- [x] #108 已运行并记录 `cargo check -p ostool`。

审查重点：

- `ProjectLayout` 创建后不可变。
- `InvocationState` 只保存 mutable runtime state。
- `Invocation` 不增长 build/run/menuconfig 方法。
- `${package}` 状态必须来自 `VariableScope`，不能来自某个过期 `BuildConfig` 副本。
- 中间阶段的 `Tool` 只能变薄，不能新增业务逻辑。

### 任务 3 / R1c：抽出 variable 和 process functions

**文件：**

- Create or modify: `ostool/src/project/variables.rs`
- Create or modify: `ostool/src/process/mod.rs`
- Optional create only when needed: `ostool/src/process/command.rs`
- Optional create only when needed: `ostool/src/process/shell.rs`
- Modify: `ostool/src/utils.rs`
- Modify: `ostool/src/run/qemu.rs`
- Modify: `ostool/src/run/uboot.rs`
- Modify: `ostool/src/board/config.rs`

步骤：

- [x] 把 `${workspace}`、`${workspaceFolder}`、`${package}`、`${tmpDir}`、`${env:VAR}` 替换移到 `project::variables`。
- [x] variable expansion 接收 `&VariableScope`；无 active build 时从 `ProjectLayout` 派生 default scope。
- [x] 引入 `ProcessContext`，显式保存 workdir、workspace_dir、VariableScope、kernel_elf。
- [x] 把 command construction 移到 `process` 模块；当前保留在 `process/mod.rs`，未过早拆 `process::command`。
- [x] 把 shell command execution 移到 `process` 模块；当前保留在 `process/mod.rs`，未过早拆 `process::shell`。
- [x] 移除旧变量替换方法，保留 `Tool::command` 作为 thin facade，调用新 `process::command`。
- [x] 保持 shell hook 的 `KERNEL_ELF` 注入语义。
- [x] Run variable/process tests。
- [x] #108 已运行并记录 `cargo check -p ostool`。

审查重点：

- 缺失环境变量仍替换为空字符串。
- `${package}` 仍在 build config 已激活时使用 selected Cargo package dir。
- command working directory 保持当前 `Tool::command` 行为。
- `WORKSPACE_FOLDER` env 行为保持不变。
- command/shell helpers 接收 concrete `ProcessContext`，不接 whole `Invocation`。
- 不创建无状态 `CommandFactory` 或 `ShellCommandRunner`。

### 任务 4 / R1d：抽出 Cargo build outcome seam

2026-05-26 实现校准：#108 先落地 `ResolvedCargoArtifact` seam；#111 已将 R1d 的剩余拆分合入上游：
`cargo_builder.rs` 改为 `cargo_pipeline.rs`，Cargo JSON executable selection 移入 `artifact_selector.rs`，
`CargoBuildPipeline::execute()` 返回 `CargoBuildOutcome`。当前 orchestration 层仍保留 legacy
`Tool` runtime state bridge，行为不变。

因此，R1d 已完成。R1e 兼容切片已经在 #111 中直接消费 `CargoBuildOutcome`。

**文件：**

- Rename: `ostool/src/build/cargo_builder.rs` -> `ostool/src/build/cargo_pipeline.rs`
- Create: `ostool/src/build/artifact_selector.rs`
- Modify: `ostool/src/build/mod.rs`

步骤：

- [x] 把 Cargo JSON executable selection 移到 `artifact_selector.rs` 并补独立测试。
- [x] 定义 `ResolvedCargoArtifact`，表达 executable path 和 Cargo artifact dir。
- [x] 定义 `CargoBuildOutcome`，由 `CargoBuildPipeline::execute()` 返回 explicit build facts。
- [x] `CargoBuildPipeline` 先解析 explicit build facts，再由 legacy orchestration 层写回
  `Tool` runtime state。
- [x] 保持 pre-build command execution order。
- [x] 保持 Cargo command arguments、features、`profile`、log feature、target dir、package、bin、extra config、`args` 和 message format。
- [x] 保持 post-build command execution order 和 `KERNEL_ELF` 注入语义。
- [x] 保持 current someboot argument behavior；R1 不修重复注入。
- [x] Run artifact selector tests。
- [x] Run `CargoBuildPipeline::execute()` outcome test。
- [x] #108 已运行并记录 `cargo test -p ostool -- --nocapture`。
- [x] #108 已运行并记录 `cargo check -p ostool`。

审查重点：

- Artifact selection 规则不变：
  - explicit `bin`
  - package-name binary
  - `default-run`
  - single binary
  - ambiguity error for multiple binaries
- `CargoBuildPipeline` 不知道 QEMU、U-Boot 或 board 行为。
- `CargoBuildPipeline` 不生成 runtime `.elf` / `.bin`。
- `CargoBuildPipeline` 返回 build facts；orchestration 层更新 state。
- 如果 struct 只包一层函数调用，删除 struct，保留 module function。

R1d 后续约束：

- 后续 pipeline 拆分必须接收 `&ProjectLayout`、`&InvocationOptions`、active build input 和
  `ProcessContext` 所需窄输入，而不是扩大 `Tool`。
- 后续 pipeline 或 module function 必须返回 explicit build facts，不接收 `&mut InvocationState`。

### 任务 5 / R1e：抽出 RuntimeArtifactPreparer

2026-05-29 实现校准：R1e 的上游友好兼容切片已通过 #111/#114 合入；#115 继续完成
runtime artifact state 接线。当前实际代码是：

- `artifact/runtime.rs` 已存在，提供 `RuntimeArtifactOptions`、`PreparedRuntimeArtifacts` 和
  `prepare_runtime_artifacts()` module function；没有创建无状态 `RuntimeArtifactPreparer` struct。
- `artifact/state.rs` 已存在并持有 `OutputArtifacts`；`ctx.rs` 通过 `pub use crate::artifact::state::OutputArtifacts`
  保持兼容导出。
- `prepare_runtime_artifacts()` 已接管 ELF canonicalization、arch detection、stripped runtime `.elf`
  和 optional `.bin` 生成。
- Cargo path：`build/mod.rs::apply_cargo_build_outcome()` 从 `CargoBuildOutcome` 读取
  `ResolvedCargoArtifact`，调用 runtime helper，再通过 `Tool::apply_prepared_runtime_artifacts()` 写入
  `InvocationState` 并同步旧 `Tool.ctx.artifacts`。
- Custom / `cargo-osrun` path：`Tool::prepare_elf_artifact()` 调用 runtime helper，再写入 `InvocationState`
  并同步旧 artifact state。
- Runner path：QEMU、U-Boot、board 仍通过 `Tool` 兼容门面取 artifact；`Tool::runtime_artifacts()` 生产路径
  优先读取 `InvocationState.artifacts`，旧 `Tool.ctx.artifacts` 作为兼容 fallback。

本节下方 checkbox 按“当前兼容切片”重新标注。未完成项不阻塞 PR-03，但应在 R2 或后续 R1 cleanup
中单独处理。

**文件：**

- Create: `ostool/src/artifact/mod.rs`
- Create: `ostool/src/artifact/state.rs`
- Create: `ostool/src/artifact/runtime.rs`
- Modify: `ostool/src/invocation.rs`
- Modify: `ostool/src/main.rs`
- Modify: `ostool/src/bin/cargo-osrun.rs`
- Modify: `ostool/src/run/qemu.rs`
- Modify: `ostool/src/run/uboot.rs`
- Modify: `ostool/src/board/mod.rs`

步骤：

- [x] 把 `OutputArtifacts` 移到 `artifact/state.rs`，并通过 `ctx.rs` re-export 保持兼容。
- [x] 定义 `PreparedRuntimeArtifacts`，保存 R1 仍需的旧字段语义。
- [x] 把 ELF canonicalization 和 arch detection 移到 runtime artifact helper。
- [x] 把 stripped `.elf` 和 optional `.bin` 生成移到 runtime artifact helper。
- [x] 只有 helper 需要持有 options、process context 或 cache 时，才保留 `RuntimeArtifactPreparer` struct；否则使用 module function。
- [x] 支持从 `CargoBuildOutcome` 准备 runtime artifact。
- [x] 支持从 custom ELF path 准备 runtime artifact。
- [x] 兼容完成：替换 `Tool::prepare_elf_artifact`、`Tool::set_elf_artifact_path`、`Tool::objcopy_elf`、`Tool::objcopy_output_bin` call sites。
  当前 `Tool::prepare_elf_artifact` 和 `Tool::objcopy_output_bin` 仍作为兼容门面存在，但内部已委托给
  `prepare_runtime_artifacts()` 并写入 `InvocationState`；`set_elf_artifact_path` / `objcopy_elf` 旧形态已不在当前代码中。
- [x] 保持当前 artifact 字段和更新行为：
  - `elf`
  - `bin`
  - `cargo_artifact_dir`
  - `runtime_artifact_dir`
- [x] orchestration 层把 `PreparedRuntimeArtifacts` 写入 `InvocationState`。
- [x] Run artifact unit tests。
- [x] Run `cargo check -p ostool`：2026-05-27 已运行
  `docker exec -w /workspace/GitHub/MRNIU/ostool devbox cargo check -p ostool`。

R1e 剩余工作：

- `artifact/state.rs` 已存在；剩余问题不是是否创建文件或是否写入 `InvocationState`，而是何时把
  `ctx::OutputArtifacts` re-export 和 `Tool.ctx.artifacts` 兼容桥接收掉。真正迁移应与 R2 artifact lifecycle
  一起做，而不是只为移动文件改 public API。
- 让 QEMU、U-Boot、board runner 的最小内部 seam 接收 prepared runtime artifact 或窄 artifact state，
  逐步减少对 `Tool` 兼容门面的读取。
- 在 PR-03 前补一个窄的 artifact state / debug artifact registry 设计，明确 runtime `.bin` 与
  future debug artifacts 的字段和路径语义。

审查重点：

- 本任务不引入 debug artifacts。
- Runtime `.bin` 仍表示 runner-consumable raw binary。
- object tool command 仍是 `rust-objcopy`。
- `debug` 仍按当前方式控制 `--strip-all`。
- runtime preparation 由 orchestration 层在 build outcome 已知后调用。
- Cargo artifact selection 和 runtime conversion 必须保持解耦。
- 不为了名字对称创建无状态 preparer 对象。

### 任务 6 / R1f：重接 CLI/build 调用边界，并替换 build config loading 和 menu hooks

2026-05-29 实现校准：#114 已完成 R1f 的一部分前置拆分；#115 继续完成 active build
和 invocation state 接线。当前 `main.rs` 和 `cargo-osrun` 已先创建 `Invocation`，随后创建兼容 `Tool`；
build config loading 已进入 `config_loader`，`.build.toml` 的 package/features/target menu hooks 已进入
`config_hooks`。CLI override 后的最终 build state 通过 `ActiveBuildContext` 表示，custom/Cargo build
orchestration 会写入 `InvocationState`，旧 `AppContext` 只作为兼容镜像保留。

**文件：**

- Create: `ostool/src/build/config_loader.rs`
- Create: `ostool/src/build/config_hooks.rs`
- Modify: `ostool/src/build/mod.rs`
- Modify: `ostool/src/main.rs`
- Modify: `ostool/src/menuconfig.rs`

步骤：

- [x] `main.rs` 和 `cargo-osrun` 开始创建 `Invocation`。
- [x] build path 从 `Invocation` / `ActiveBuildContext` / helper functions 接线，不再通过 `Tool` 作为业务状态中心。
- [x] custom build 和 Cargo build 都通过 orchestration 层更新 `InvocationState`。
- [x] 把 build config path resolution 和 `jkconfig::run` 用法移到 `BuildConfigLoader` 或 module functions。
- [x] 把 package/features/target hooks 移到 `config_hooks.rs`。
- [x] loader 在 CLI override 后创建 `ActiveBuildContext`。
- [x] relative `extra_config` 仍按 build config path parent 解析。
- [x] menuconfig 保持现有 hooks 行为，不顺手重写交互逻辑。
- [x] Run config/menu hooks 相关 tests：2026-05-27 已运行
  `docker exec -w /workspace/GitHub/MRNIU/ostool devbox cargo test -p ostool --lib -- --nocapture`。
- [x] Run `cargo check -p ostool`：2026-05-27 已运行
  `docker exec -w /workspace/GitHub/MRNIU/ostool devbox cargo check -p ostool`。

审查重点：

- `ActiveBuildContext` 是 CLI override 后的最终态。
- `VariableScope` 从 `ActiveBuildContext` 派生。
- loader 不知道 QEMU/U-Boot/board 执行。
- someboot cleanup 仍留到 R2。
- `Tool` 如果还存在，只能作为旧 API 兼容门面，不再是 CLI 主路径。

### 任务 7 / R1g：围绕显式输入重写 runner 和 board entrypoints

2026-05-27 实现校准：当前尚不具备直接执行完整 R1g 的干净前置状态。QEMU、U-Boot、board
仍有跨模块 `impl Tool`，`QemuRunner` / U-Boot `Runner` 仍持有或接收 `Tool`，board run 仍通过
`Tool::run_board()` 准备 runtime artifact 并调用 remote U-Boot path。可以先做 R1g 的窄前置 seam，
但不应跳过 R1f 后半段直接做完整 runner/board rewrite。

**文件：**

- Modify: `ostool/src/run/qemu.rs`
- Modify: `ostool/src/run/uboot.rs`
- Modify: `ostool/src/run/tftp.rs`
- Modify: `ostool/src/board/mod.rs`
- Modify: `ostool/src/board/config.rs`
- Modify: `ostool/src/main.rs`
- Modify: `ostool/src/bin/cargo-osrun.rs`

步骤：

- [ ] 用 explicit functions 或 `QemuRunner` 替换 QEMU 的 `impl Tool` blocks。
- [ ] 用 explicit functions 或 `UbootRunner` 替换 U-Boot 的 `impl Tool` blocks。
- [ ] 用 `BoardRunner` 或 explicit board functions 替换 board `impl Tool` run methods。
- [ ] runner config readers 接收 `ProjectLayout`、`VariableScope`、`ProcessContext` 和 concrete artifact/state inputs。
- [ ] QEMU/U-Boot execution 显式接收 prepared runtime artifact state。
- [ ] 保持 QEMU `to_bin`、default machine、`--dtb-dump`、`-kernel`、UEFI pflash/ESP、output matcher 行为。
- [ ] 保持 U-Boot local/remote backend、FIT generation、TFTP/YMODEM staging、post-run command 行为。
- [ ] 保持 board session acquire/retry/heartbeat/release 行为。
- [ ] Run `cargo test -p ostool qemu_byte_stream`。
- [ ] Run `cargo check -p ostool`。

审查重点：

- 没有真实 board/server 证据时，不声称 board 硬件行为已验证。
- 不引入泛化 `RunContext` 取代 `Tool`。
- 如果 helper 依赖很多输入，按职责拆分，而不是扩大对象。
- 这是副作用风险最高的切片；前面 seam 未稳定前不要提前做。

### 任务 8 / R1h：移除 `Tool` 并清理导出

**文件：**

- Delete: `ostool/src/tool.rs`
- Delete: `ostool/src/ctx.rs`
- Modify: `ostool/src/lib.rs`
- Modify: `ostool/tests/public_api.rs`
- Delete or rewrite: `ostool/tests/ui/pass_tool_configs.rs`
- Modify: `ostool/tests/ui/*.stderr`

步骤：

- [ ] Remove `mod tool`。
- [ ] Remove `pub use tool::{ManifestContext, Tool, ToolConfig, resolve_manifest_context}`。
- [ ] Remove or intentionally replace public `ctx` module and `OutputArtifacts` export。
- [ ] 如果选择 Upstream-friendly mode，提供 deprecated compatibility wrappers/re-exports。
- [ ] Export only intended modules and public types。
- [ ] Remove all `use crate::Tool` imports。
- [ ] Run `rg -n "Tool|ToolConfig|ManifestContext|AppContext|impl Tool" ostool/src ostool/tests`。
- [ ] 确认剩余命中只是不影响代码的历史注释，或者没有剩余命中。
- [ ] Run `cargo test -p ostool public_api`。
- [ ] Run `cargo check -p ostool`。

审查重点：

- Rust API break 在这里变得可见，包括 `ostool::ctx::OutputArtifacts`。
- Fork-side mode 下不要留下 deprecated `Tool` wrapper。
- Upstream-friendly mode 下 wrapper 必须 delegate 到 `Invocation` 和 services，不能成为新业务中心。
- 到这个切片前，`Tool` 应该已经很薄；如果删除仍需要大规模改业务逻辑，说明前面切片没有完成。

### 任务 9 / R1i：最终验证和文档同步

**文件：**

- Modify: `.Codex/fork-only/2026-05-14-ostool-architecture-refactor-plan.md`
- Optionally modify: `README.md`, `README.en.md` only if user-visible CLI/config docs change

步骤：

- [ ] Run `cargo fmt --all -- --check`。
- [ ] Run `cargo check -p ostool`。
- [ ] Run `cargo test -p ostool`。
- [ ] 如果需要 Docker/CI-equivalent validation，使用 R0 Docker notes，不安装 host dependencies。
- [ ] 更新 fork-only architecture plan 的 R1 completion note。
- [ ] 记录任何无法运行的检查。

审查重点：

- CLI/config 行为未变时，README 不应变化。
- compile-fail output 如果只因内部 API 名称变化而变，记录为 intentional。
- 不要声称 full repository test green，除非已单独解决或 CI 证明 `ostool-server` WebSocket lifecycle hang 稳定。

## 8. 横向审查矩阵

| 角度 | 审查问题 | 必须成立 |
|---|---|---|
| Architecture | `Tool` 是否真的消失为业务中心？ | 无跨模块 `impl Tool`，无替代 god context |
| Naming | 名字是否表达领域角色？ | `ProjectLayout`、`Invocation`、`InvocationState`、`Pipeline`、`Runner`、`Preparer`、`Loader`、`Resolver` |
| Behavior | CLI/config/runtime 行为是否变化？ | R1 无用户可见行为变化 |
| Rust API | API break 是否有意且被记录？ | `Tool` break documented；其它 public shrink 不应顺手发生 |
| Borrowing | 是否避免长期复杂 mutable borrow？ | 优先函数参数和短 borrow scope |
| State | mutable state 是否显式？ | `InvocationState` 只由 orchestration 层更新 |
| Encapsulation | subsystem 能否随意改 core objects？ | core fields private；通过窄方法更新 |
| Variable Scope | selected package 是否单一真相源？ | CLI override 后 `${package}` 来自 `VariableScope` |
| Process | command/shell 语义是否完整保留？ | workdir、`WORKSPACE_FOLDER`、replacement、`KERNEL_ELF` 均有测试 |
| Artifact | runtime artifacts 是否仍区别于未来 debug artifacts？ | R1 保持当前字段语义；R2 处理生命周期升级 |
| Build | Cargo artifact selection 是否稳定？ | 同样 resolution/ambiguity behavior；runtime conversion 在 pipeline 外 |
| Runner | QEMU/U-Boot 副作用是否保持？ | R1 不重写 command plan 或 FIT 行为 |
| Board | 硬件安全是否保持？ | acquire/retry/heartbeat/release 不弱化；无假硬件证明 |
| Tests | 检查是否证明行为？ | parser、selector、variable/process、artifact、compile、QEMU byte-stream；无 source-grep 测试 |
| Upstream PR | PR 是否可 review？ | 一个架构主题；无 SimpleKernel-specific feature；API break 策略前置 |

## 9. 风险复核

### R1 风险：Scope Creep 到 R2

触发：

- 移动 build/artifact 代码时，顺手修 debug artifacts、artifact lifecycle 或 someboot duplicate args。

缓解：

- 保持 `OutputArtifacts` 字段和行为不变。
- 保持 someboot 当前行为不变。
- 后续事项写入计划，不放进 R1 代码变更。

### R1 风险：`Invocation` 变成新 `Tool`

触发：

- 模块开始给 `Invocation` 增加 build/run/config 业务方法。

缓解：

- `Invocation` 只暴露窄访问器。
- `Invocation` 字段 private。
- service API 接收窄输入，不接整个 `Invocation`。
- business behavior 放在命名清晰的 modules/services。
- code review 拒绝包含领域逻辑的跨模块 `impl Invocation`。

### R1 风险：`InvocationState` 变成新可变中心

触发：

- build pipeline、runner、config loader 直接接收 `&mut InvocationState` 并修改共享状态。

缓解：

- build pipeline 返回 `CargoBuildOutcome`。
- runtime preparer 返回 `PreparedRuntimeArtifacts`。
- runner 读取 prepared artifact，不写 build state。
- 只有 orchestration 层负责把 outcome 写入 `InvocationState`。

### R1 风险：`VariableScope` 偏离 selected Cargo package

触发：

- CLI `--package` / `--bin` override 改了一个 `BuildConfig` 副本，variable expansion 又读另一个副本。

缓解：

- 先应用 CLI selector override，再创建 `ActiveCargoBuild`。
- `VariableScope` 从最终 selected package dir 计算一次。
- variable expansion 读取 `VariableScope`，不读取 mutable `BuildConfig`。

### R1 风险：`KERNEL_ELF` 语义被缩小

触发：

- shell helper 只按 Cargo post-build 设计，忘记当前所有 `shell_run_cmd()` 在 artifact 存在时都会注入 `KERNEL_ELF`。

缓解：

- 引入 `ProcessContext.kernel_elf`。
- shell execution 统一从 `ProcessContext` 注入 `KERNEL_ELF`。
- 增加 shell context test。

### R1 风险：Rust public API break 被低估

触发：

- `Tool` 当前从 `lib.rs` re-export，`ctx::OutputArtifacts` 也通过 `pub mod ctx` 公开。

缓解：

- 开工前选择 compatibility mode。
- Fork-side mode：提交说明明确 internal API reset。
- Upstream-friendly mode：保留 deprecated wrapper/re-export，或先确认上游接受 breaking API。

### R1 风险：测试护栏过窄

触发：

- 只补 parser tests，没有覆盖 selector、variable/process、artifact behavior。

缓解：

- 任务 1 前置补行为测试。
- 不用 source-text grep 冒充 contract test。
- 对当前没有纯 seam 的 QEMU command plan，不在 R1 伪造测试；保留 QEMU byte-stream integration smoke。

### R1 风险：Nominal services 增加样板而非解耦

触发：

- 创建很多无状态 `*Service`、`*Factory`、`*Runner`，只是把函数套一层。

缓解：

- 没有状态、资源或测试 seam 时用 module-level functions。
- trait 只在有多个实现或 concrete mock boundary 时引入。
- review 重点看 dependency direction 是否改变，而不是文件名是否好看。

## 10. 实施前 checklist

- [ ] 执行者已读 `AGENTS.md`。
- [ ] 执行者已读 `ostool/AGENTS.md`。
- [ ] 执行者已读本 R1 plan。
- [ ] 执行者已读 R0 contract baseline。
- [ ] 执行者已检查 `git status`。
- [ ] 执行者已确认是创建 branch 还是继续当前 branch。
- [ ] 执行者已确认 compatibility mode：Fork-side mode 或 Upstream-friendly mode。
- [ ] 执行者理解：Rust `Tool` API break 在 Fork-side mode 下允许，CLI/config/runtime 行为不允许改变。
- [ ] 执行者理解：`OutputArtifacts` / `ostool::ctx` 包含在 R1 internal API reset 中，除非明确选择上游兼容 wrapper。
- [ ] 执行者理解：R2/R3 feature work 不属于 R1。

## 11. 建议分支和提交形态

建议分支：

```bash
git switch -c feature/invocation-architecture
```

2026-05-23 之后，如果目标是继续向上游提交，优先从最新 `upstream/main` 创建新的
`feature/...` 分支，并只提交当前 follow-up 切片。不要复用已经合入的
`feature/invocation-boundaries-upstream`，也不要从包含 fork-only 文档的本地历史直接开上游 PR。

如果变更过大，建议拆成以下提交：

1. `test(ostool): 补充 R1 架构重构前行为护栏`
2. `refactor(ostool): 引入 invocation 与 project layout`
3. `refactor(ostool): 抽出变量替换和进程上下文`
4. `refactor(ostool): 抽出 Cargo build outcome 边界`
5. `refactor(ostool): 抽出 runtime artifact 准备服务`
6. `refactor(ostool): 改造 build config loader 和 menu hooks`
7. `refactor(ostool): 改造 runner 和 board 调用边界`
8. `refactor(ostool): 移除 Tool 中心化 API`

如果上游 reviewer 更偏好单提交，等全部检查通过后再 squash。

上游友好模式下，提交 5-8 不应被视为必须连续提交的一组。特别是 `Tool` removal 应等前面的
显式输入边界、artifact state 和 runner entrypoint 已经稳定后再讨论。

## 12. 完成判据

### 12.1 当前上游检查点：#108 / #111 / #114 / #115

#108 只能标记为 “R1 upstream-friendly seed 已合入”。#111 在此基础上合入了 R1d/R1e 的兼容切片。
#114 进一步收敛了 artifact state、build config loader 和 menu hooks。#115 继续把
`InvocationState` / `ActiveBuildContext` 接入生产路径。它们共同满足：

- invocation/project/process 的初始边界已经进入上游。
- Cargo executable selection 已从 build pipeline 中拆出。
- `CargoBuildPipeline::execute()` 返回 `CargoBuildOutcome`，不直接写旧 artifact state。
- `CargoBuildPipeline` 不再持有 `&mut Tool`，而是接收显式 `CargoBuildInput`。
- runtime artifact preparation 已从 Cargo pipeline /旧 objcopy 流程中拆到 `artifact/runtime.rs`。
- `OutputArtifacts` 已迁入 `artifact/state.rs`，并由 `InvocationState` 持有；`ctx.rs` 仍 re-export
  兼容 API。
- `.build.toml` loader 和 menu hooks 已进入 `build/config_loader.rs` / `build/config_hooks.rs`。
- CLI override 后的 build state 通过 `ActiveBuildContext` 表示，`VariableScope` 优先从 active build 派生。
- `PreparedRuntimeArtifacts` 写入 `InvocationState`，再同步旧 `AppContext`。
- CLI/config/runtime 行为按 PR 验证保持兼容。
- `Tool` 继续作为兼容门面，但不再是唯一 runtime artifact state owner。

它不满足下面的完整 R1 完成判据。

### 12.2 完整 R1 终态

R1 完成时必须满足：

- `Tool`、`ToolConfig`、`ManifestContext`、`AppContext` 和 `ctx::OutputArtifacts` 不再是核心模型名。
- `ostool/src/tool.rs` 和 `ostool/src/ctx.rs` 被移除，或被新架构完全替代。
- `Invocation` 存在，但不承载业务行为。
- core invocation fields private，mutation 通过窄方法完成。
- selected build/package state 通过 `ActiveBuildContext` 和 `VariableScope` 表示。
- `ProcessContext` 显式承载 workdir、workspace、variable scope 和 optional `KERNEL_ELF`。
- Cargo executable resolution 返回明确的 build fact，例如当前的 `ResolvedCargoArtifact`
  或后续真实需要的 outcome 类型；build pipeline 不直接写 `InvocationState`。
- runtime `.elf` / `.bin` preparation 独立于 Cargo build pipeline。
- business behavior 位于明确模块和服务中。
- 不再有跨模块 `impl Tool`。
- CLI/config/runtime 行为保持不变。
- `cargo check -p ostool` 通过。
- `cargo test -p ostool` 通过，或任何失败都有准确原因和影响范围记录。
- 最终 PR 或提交说明写明这是围绕 invocation architecture 的 internal API reset。
