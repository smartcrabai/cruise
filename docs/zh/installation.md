[English](../en/installation.md) · [日本語](../ja/installation.md) · [简体中文](../zh/installation.md) · [文档索引](README.md)

<a id="prerequisites"></a>
## 前置条件

- [`gh` CLI](https://cli.github.com/) —— 启用自动创建 PR（默认设置）时，GitHub worktree 会话、基于 PR 的清理或显式启用的 workflow 合并都需要它。GitLab 仓库使用 `glab`。设置了 `create_pr: false` 的本地 worktree workflow 不需要 forge CLI，但 `--repo` 仍需要 `gh`/`glab` 来访问并克隆仓库。
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) —— GitLab 仓库需要用它代替 `gh`（用于创建合并请求、清理、发布 issue，以及显式启用的原生同步 squash 合并）。系统根据主机名（`github.com` / `gitlab.com`）检测 forge；对于自托管主机，设置 `CRUISE_FORGE=github|gitlab`；对于 `owner/repo` 格式的 `--repo` 定位符，还需设置 `GITLAB_HOST`。`--repo` 接受 `owner/repo`、嵌套的 `group/sub/project`（需 `CRUISE_FORGE=gitlab`）或完整 URL。在 GitLab 上，`{pr.number}` / `{pr.url}` 指的是合并请求；发布 issue 时的“发布 `@cruise run` 评论”选项不可用。
- [`jcode` CLI](https://github.com/1jehuang/jcode) —— 默认 SDK 后端（`sdk: jcode`）所必需；安装后使用 `jcode login <provider>` 登录。工作流使用 jcode SDK 时需要安装。
- 若要使用 `cruise ssh`，本地 `PATH` 中必须有 OpenSSH 客户端（`ssh`）。
- Windows 需要 [Git for Windows](https://gitforwindows.org/)（`git` 必须位于 `PATH` 中）。
- SSH 目标主机上必须有兼容版本的 `cruise` 及所选工作流所需的常规前置条件，例如需要时安装 `gh` 和 `jcode`。工作流由远端主机执行，远端主机负责其认证、配置、会话、worktree 和克隆副本。
- 例如，可在目标主机上安装托管 jcode CLI：`cruise ssh <host> -- install jcode`。

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

shell 和 PowerShell 安装程序会将 `cruise` 安装到 `~/.local/bin`（Windows 上为 `%USERPROFILE%\.local\bin`），不受 `CARGO_HOME` 设置影响。请确保该目录位于 `PATH` 中。受支持的托管安装可使用 `cruise update`；参见下文[更新 Cruise](#updating-cruise)。若要安装固定版本，请将安装程序 URL 中的 `/releases/latest/` 替换为 `/releases/download/v<version>/`，例如 `v0.2.15`。Homebrew 安装应继续由 Homebrew 管理。

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
| 使用 shell 或 PowerShell 安装程序安装到 `~/.local/bin`（Windows 上为 `%USERPROFILE%\.local\bin`），或现有的规范 `$CARGO_HOME/bin` 安装 | `cruise update` |
| 在规范的 `$CARGO_HOME/bin` 中从源码构建安装 | `cruise update`；它会安装官方 runtime，而非本地源码构建版本或其构建时更改 |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action、Docker 镜像或其他外部安装 | 使用其原始分发方式更新 |
| 设置了 `CRUISE_RUNTIME` 的任何安装 | 取消设置该覆盖项，然后从托管 launcher 运行 `cruise update` |

“托管”由可执行文件的位置决定：安装程序默认目录 `~/.local/bin`（Windows 上为 `%USERPROFILE%\.local\bin`）或规范的 `$CARGO_HOME/bin`，而非 Cargo 的安装账本。更新程序只替换 runtime 选择：固定的 `cruise` launcher（Windows 上为 `cruise.exe`）保持不变；每次更新都会添加带版本号的 `cruise-runtime-managed-…` runtime 和 `.json` sidecar，并切换 `.cruise-runtime-active`。旧 runtime 会保留。原生更新不会运行 Cargo，也不会更改其安装账本。如果需要自定义的源码构建 runtime，请从源码重新构建并安装。

执行 `cruise update` 后，下一个客户端请求会使用新 runtime 自动重启空闲 daemon。daemon 忙碌时会继续运行旧版本：CLI 客户端会拒绝请求，并提示等待空闲后运行 `cruise daemon restart`；重启前 WebUI 也会继续使用旧版本。使用 `cruise daemon status` 检查 daemon；不要再使用已移除的 `cruise webui status` 或 `restart` 命令。

<a id="docker"></a>
### Docker

将 Docker 镜像作为长期运行的 service container 启动，并通过 `--env-file` 等方式向该容器提供凭据及其他进程级环境变量：

```sh
docker run -d --name cruise -p 127.0.0.1:8484:8484 --env-file .env cruise:latest
```

`docker logs cruise` 会显示 WebUI 登录 URL。URL 中是五分钟后过期、只能使用一次的 nonce，而不是 registry API token。除非确实需要网络访问，否则请仅将服务发布到 loopback（`-p 127.0.0.1:8484:8484`）；daemon 使用明文 HTTP。请在同一个 service container 中运行 CLI 命令。

```sh
docker exec cruise cruise plan "implement the feature"
```

一次性容器（例如 `docker run --rm --env-file .env cruise:latest plan "implement the feature"`）可在前台正常运行。命令结束后容器会停止，daemon 也随之停止，因此不要用一次性容器执行后台 `--plan` 或 detached work。请通过 `--env-file` 或 `-e` 将凭据和进程级环境变量提供给 service container。

<a id="managed-provider-clis"></a>
### 托管的 jcode CLI

安装由 Cruise 管理的 jcode CLI，或检查其状态：

```sh
cruise install jcode
cruise install --status
cruise install jcode --force
```

每次安装都会查询 jcode 最新稳定版的发布元数据。若托管安装已是所选版本且状态正常，则会复用；若为其他版本，则会更新。`--force` 会重新下载所选的最新版本。

安装需要联网访问发布元数据和下载文件，上游变更可能影响兼容性。下载文件会使用发布者提供的校验和进行验证，但这不是独立签名。`cruise install --status` 可离线运行，只报告记录的已安装版本，不声称该版本是最新的。托管运行时也根据记录的实际安装版本进行验证，而非依据 Cruise 构建固定的版本。
