[English](../en/installation.md) · [日本語](../ja/installation.md) · [简体中文](../zh/installation.md) · [文档索引](README.md)

<a id="prerequisites"></a>
## 前置条件

- [`gh` CLI](https://cli.github.com/) —— 当启用自动创建 PR（默认设置）时，GitHub worktree 会话、基于 PR 的清理或选择启用的原生 workflow 合并都需要它；GitLab 仓库则使用 `glab`。设置了 `create_pr: false` 的本地 worktree workflow 不需要 forge CLI，但 `--repo` 仍需要 `gh`/`glab` 来访问并克隆仓库。
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) —— GitLab 仓库需要用它代替 `gh`（用于创建合并请求、清理、发布 issue，以及选择启用的原生同步 squash 合并）。系统根据主机名（`github.com` / `gitlab.com`）检测 forge；对于自托管主机，设置 `CRUISE_FORGE=github|gitlab`（对于 `owner/repo` 格式的 `--repo` 定位符，还需设置 `GITLAB_HOST`）。`--repo` 接受 `owner/repo`、嵌套的 `group/sub/project`（需 `CRUISE_FORGE=gitlab`）或完整 URL。在 GitLab 上，`{pr.number}` / `{pr.url}` 指的是合并请求；发布 issue 时的“发布 `@cruise run` 评论”选项不可用。
- [`jcode` CLI](https://github.com/1jehuang/jcode) v0.88.0 或更新版本 —— 默认 SDK 后端（`sdk: jcode`）所必需；安装后使用 `jcode login <provider>` 登录。若某个有效的执行或规划候选项使用 jcode（包括其他 workflow SDK 下标记为 SDK 的数组项），则 SDK 执行需要安装 jcode。
- [`claude` CLI](https://code.claude.com/docs/en/quickstart) —— 只要某个有效的执行或规划候选项使用 claude（包括标记为 SDK 的数组项），就必须安装。Cruise 在进程内驱动它。已针对 2.1.250 验证；`:effort` 模型后缀会映射到 `claude --effort`，因此不支持该参数的 CLI 会以 `unknown option '--effort'` 使步骤失败（这是永久性错误，不会重试）。其认证由 CLI 自行处理，与 `jcode login` 无关。
- `opencode` CLI（V2 API）—— 只要某个有效的执行或规划候选项使用 opencode（包括标记为 SDK 的数组项），就必须安装。Cruise 会在本地启动和停止它，并使用现有的 OpenCode provider 配置与认证。不支持 V1。
- 若要使用 `cruise ssh`，本地 `PATH` 中必须有 OpenSSH 客户端（`ssh`）。
- Windows：需要 [Git for Windows](https://gitforwindows.org/)（`git` 必须位于 `PATH` 中）。`command:` 步骤通过 `cmd.exe /C` 运行，因此请使用 cmd.exe 语法（或显式调用 `powershell -Command ...` / `bash -c ...`），不要使用 POSIX shell 语法。桌面通知和通知声音不可用，但会发送已配置的 webhook。`cruise ssh` 需要 Windows OpenSSH 客户端；herdr 报告功能仅限于 herdr 环境变量所能提供的能力。
- SSH 目标主机上必须有兼容版本的 `cruise` 及所选 workflow 的常规前置条件，包括所需的 `gh`、`jcode`、`claude` 或 `opencode` CLI。workflow 由远端主机执行，远端主机负责其认证、配置、会话、worktree 和克隆副本。
- 例如，可在该主机上安装托管 provider：`cruise ssh <host> -- install jcode`。

<a id="installation"></a>
## 安装

<a id="windows-powershell"></a>
### Windows（PowerShell）

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.ps1 | iex"
```

<a id="macos-and-linux-shell-installer"></a>
### macOS 和 Linux（shell 安装程序）

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh | sh
```

或者先将安装程序下载到文件，只有下载成功后才运行：

```sh
curl --proto '=https' --tlsv1.2 -fLsS --output cruise-installer.sh \
  https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh \
  && sh ./cruise-installer.sh
```

shell 和 PowerShell 安装程序会将 `cruise` 安装到 `$CARGO_HOME/bin`（通常为 `~/.cargo/bin`；Windows 上若未设置 `CARGO_HOME`，则为 `%USERPROFILE%\.cargo\bin`）。请确保该目录位于 `PATH` 中。受支持的托管安装可使用 `cruise update`；参见下文[更新 Cruise](#updating-cruise)。若要安装固定版本，请将安装程序 URL 中的 `/releases/latest/` 替换为 `/releases/download/v<version>/`，例如 `v0.2.15`。Homebrew 安装应继续由 Homebrew 管理。

<a id="homebrew"></a>
### Homebrew

```sh
brew install smartcrabai/tap/cruise
```

使用以下命令升级 Homebrew 安装：

```sh
brew upgrade smartcrabai/tap/cruise
```

<a id="updating-cruise"></a>
### 更新 Cruise

对于受支持的托管安装，请在更新前后检查版本：

```sh
cruise --version; cruise update; cruise --version
```

`cruise update` 会获取最新的官方稳定版。如果该版本与已安装版本相同或更旧，则不会执行任何操作。原生更新支持以下发布目标：`aarch64-apple-darwin`、`x86_64-apple-darwin`、`aarch64-unknown-linux-gnu`、`x86_64-unknown-linux-gnu` 和 `x86_64-pc-windows-msvc`。

| 安装方式 | 更新方法 |
|---|---|
| 通过 shell 或 PowerShell 安装程序安装到规范的 `$CARGO_HOME/bin` | `cruise update` |
| 在规范的 `$CARGO_HOME/bin` 中从源码构建安装 | `cruise update`；它会安装官方 runtime，而非本地源码构建版本或其构建时更改 |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action、Docker 镜像或其他外部安装 | 使用其原始分发方式更新 |
| 设置了 `CRUISE_RUNTIME` 的任何安装 | 取消设置该覆盖项，然后从托管 launcher 运行 `cruise update` |

“托管”由规范的 `$CARGO_HOME/bin` 位置决定，而非 Cargo 的安装账本。更新程序只替换 runtime 选择：固定的 `cruise` launcher（Windows 上为 `cruise.exe`）保持不变；每次更新都会添加带版本号的 `cruise-runtime-managed-…` runtime 和 `.json` sidecar，并切换 `.cruise-runtime-active`。旧 runtime 会保留。原生更新不会运行 Cargo，也不会更改其安装账本。如果需要自定义的源码构建 runtime，请从源码重新构建并安装。

不含更新程序的已发布版本（包括 v0.2.16）无法自行更新。支持更新的构建可以选择 v0.2.16，但该已发布 runtime 无法再执行后续原生更新。可通过平台安装程序安装启用了更新功能的版本，或从对应版本源码重新构建并安装来完成引导；只有已安装的 runtime 包含更新程序时，之后才能使用 `cruise update`。

下载的发布包通过 HTTPS 获取，并根据发布清单中的 SHA-256 校验和进行检查。校验和由同一发布方提供，并非独立的数字签名。

`cruise update --status` 会按排序顺序列出所有安装记录的事务。它是只读的：不会访问网络、获取更新锁、修复状态，也不能证明当前激活的是哪个 runtime。事务文件位于 `$XDG_STATE_HOME/cruise/installations/<installation-id>/update-transaction.json`（默认 `~/.local/state/cruise/installations/<installation-id>/update-transaction.json`；Windows 上若未设置 `XDG_STATE_HOME`，则为 `%LOCALAPPDATA%\cruise\state\installations\<installation-id>\update-transaction.json`）。失败的事务会保留其阶段和错误。重试 `cruise update`；它会在开始另一次下载前协调中断的事务。激活前的失败会保留旧选择。如果激活后的 launcher 探测失败，Cruise 会恢复先前的选择；如果恢复过程无法确认所选 runtime，请保留记录，并根据报告的错误手动恢复，而不要删除事务文件。

如果 Cruise 报告更新已应用但事务完成或清理时发出警告，则新 runtime 已经激活；更新并未回滚。检查 `cruise update --status`，然后重试 `cruise update` 以恢复或清理记录的事务。状态阶段只是元数据，不能证明 runtime 已激活；请使用 `cruise --version` 检查正在运行的版本。

更新 Cruise 不会替换已经运行的 WebUI 进程中的代码。更新后，使用 `cruise webui status` 将其版本与当前 runtime 比较，再使用 `cruise webui restart` 加载更新后的 runtime。重启会取消活跃的 WebUI 操作，请选择合适的时间。

多个带版本号的 runtime 可以共存于 `$CARGO_HOME/bin`（Windows 默认是 `%USERPROFILE%\.cargo\bin`）。若要固定到一个已安装版本，请使用 `--version` 验证该精确版本的同目录文件，确认相邻的 `.json` sidecar 属于当前固定 launcher/runtime 基线，然后将 `.cruise-runtime-active` 设置为该 runtime 的精确基本文件名。不要删除其他 runtime 文件。基线是固定 launcher/runtime 的大小与修改时间戳：重新安装若更改了基线，就会使 overlay 失效；大小和修改时间完全相同的替换则检测不到。旧版指针或固定到较旧 launcher 可能仍需手动退役指针。退役指针之前，先直接验证固定的同目录 runtime；之后才归档并移动指针：

```sh
# macOS / Linux
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
archive="$bin/.cruise-runtime-active.archive"
retired="$bin/.cruise-runtime-active.retired"
test ! -e "$archive" && test ! -e "$retired" &&
  "$bin/cruise-runtime" --version &&
  cp -n "$bin/.cruise-runtime-active" "$archive" &&
  mv -n "$bin/.cruise-runtime-active" "$retired" &&
  "$bin/cruise" --version
```

```powershell
# Windows
$bin = if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } else { Join-Path $HOME '.cargo\bin' }
$runtime = Join-Path $bin 'cruise-runtime.exe'
$pointer = Join-Path $bin '.cruise-runtime-active'
$launcher = Join-Path $bin 'cruise.exe'
$archive = "$pointer.archive"
$retired = "$pointer.retired"
if ((Test-Path -LiteralPath $archive) -or (Test-Path -LiteralPath $retired)) { throw 'Choose unused archive and retired paths.' }
& $runtime --version
if (-not $? -or $LASTEXITCODE -ne 0) { throw 'The fixed sibling runtime did not pass --version; pointer was not moved.' }
Copy-Item -LiteralPath $pointer -Destination $archive -ErrorAction Stop
Move-Item -LiteralPath $pointer -Destination $retired -ErrorAction Stop
& $launcher --version
```

上面的第一个 `--version` 必须成功，之后才能移动指针。这些命令只归档并移动指针文件；绝不能删除 runtime 或 sidecar。如果这些文件已存在，请选择未使用过的归档/退役名称。

<a id="managed-provider-clis"></a>
### 托管的 provider CLI

只安装 workflow 所需的 provider：

```sh
cruise install jcode
cruise install claude
cruise install opencode
cruise install --status
cruise install jcode --force
```

此构建固定使用 jcode **0.99.1**、claude **2.1.250** 和 opencode **2.1.3**。下载内容会根据固定在 Cruise 中的摘要进行校验，并在发布前检查解压后可执行文件的版本。无需使用供应商安装程序、npm CLI，也不会更改 shell 配置。托管文件位于 `$XDG_DATA_HOME/cruise/providers/<provider>/`（通常为 `~/.local/share/cruise/providers/`）。每次安装使用单独的带版本目录，当前活动路径以相对路径形式记录在 `install.json` 中。旧目录会保留，以便并发读取者继续使用已经解析出的路径。具体可用性取决于平台。认证仍由 provider 负责。

如果 jcode 仅由 Cruise 安装，请在 Cruise 数据目录下的 `providers/jcode/install.json` 中找到其活动的 `relative_path`，然后用该二进制执行 `login <provider>` 或 `auth status`。Windows 上，如果未设置 `XDG_DATA_HOME` 且已设置 `LOCALAPPDATA`，默认数据目录为 `%LOCALAPPDATA%\cruise\data`。如果 `LOCALAPPDATA` 不可用，则使用 home 目录回退路径。路径包含空格时请加引号。

解析时优先使用非空的进程环境覆盖项，其次使用与当前固定版本匹配且有效的托管安装，最后查找现有的 PATH。覆盖项为 `CRUISE_JCODE_CLI_PATH`、`CRUISE_CLAUDE_CLI_PATH` 和 `CRUISE_OPENCODE_CLI_PATH`。显式覆盖路径无效时会报错，不会回退。这些是进程环境变量，而非 workflow `env:` 设置。缺失、过期或损坏的托管安装会回退到 PATH。运行 workflow、规划、TUI 和 WebUI 时都不会下载或安装 provider。

`--status` 为只读操作：不会访问网络、执行 provider 二进制文件或创建文件。需要时使用 `--force` 重新安装。Cruise 更新后若固定版本发生变化，请重新运行 `cruise install <provider>`。`cruise update` 更新的是 Cruise 本身，而非 provider。对于托管的 claude 和 opencode，只有相应环境变量原先未设置时，Cruise 才会禁用其自动更新程序。通过 PATH 或覆盖项安装的版本保留原有行为。GitHub Action 仍会单独管理其 jcode 安装和版本输入。
