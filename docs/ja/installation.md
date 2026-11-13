**言語:** [English](../en/installation.md) | [日本語](../ja/installation.md) | [简体中文](../zh/installation.md) · [目次](README.md)

<a id="prerequisites"></a>
## 前提条件

- [`gh` CLI](https://cli.github.com/) — GitHub の worktree セッションで自動 PR 作成（既定）、PR に基づくクリーンアップ、または明示的に有効化したワークフロー内マージを使う場合に必要です。GitLab リポジトリでは `glab` を使用します。`create_pr: false` のローカル worktree ワークフローには forge CLI は不要ですが、`--repo` ではリポジトリへのアクセスとクローンのため `gh`/`glab` が必要です。
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) — GitLab リポジトリでは `gh` の代わりに必要です（merge request の作成、クリーンアップ、issue の公開、明示的に有効化したネイティブ同期 squash merge）。forge はホスト名（`github.com` / `gitlab.com`）から判定されます。セルフマネージドホストでは `CRUISE_FORGE=github|gitlab` を設定し、`owner/repo` 形式の `--repo` ロケーターでは `GITLAB_HOST` も設定します。`--repo` は `owner/repo`、ネストされた `group/sub/project`（`CRUISE_FORGE=gitlab` の場合）、または完全な URL を受け付けます。GitLab では `{pr.number}` / `{pr.url}` は merge request を指し、issue 公開時に `@cruise run` コメントを投稿するオプションは使用できません。
- [`jcode` CLI](https://github.com/1jehuang/jcode) — 既定の SDK バックエンド（`sdk: jcode`）で必要です。インストール後に `jcode login <provider>` でサインインしてください。SDK 実行では、別のワークフロー SDK の下にある SDK 指定配列項目も含め、実行または計画の有効な候補が jcode を使う場合にインストールしてください。
- [`claude` CLI](https://code.claude.com/docs/en/quickstart) — 実行または計画の有効な候補が claude を使う場合に必要です（SDK 指定配列項目を含む）。Cruise はプロセス内で claude を実行します。モデル名の `:effort` 接尾辞は `claude --effort` に対応するため、このフラグのない CLI ではステップが `unknown option '--effort'` で失敗します。認証は Claude 独自のもので、`jcode login` とは無関係です。
- V2 API 対応の `opencode` CLI — 実行または計画の有効な候補が opencode を使う場合に必要です（SDK 指定配列項目を含む）。Cruise はローカルで起動・停止し、既存の OpenCode プロバイダー設定と認証を使用します。V1 はサポートされません。
- `cruise ssh` 用に、ローカルの `PATH` 上に OpenSSH クライアント（`ssh`）が必要です。
- Windows では [Git for Windows](https://gitforwindows.org/)（`PATH` 上の `git`）が必要です。
- SSH 接続先には、互換性のある `cruise` と選択したワークフローに必要な通常の前提条件（必要に応じて `gh`、`jcode`、`claude`、`opencode` CLI など）が必要です。ワークフローはリモートホストで実行され、認証、設定、セッション、worktree、クローンもそのホストが管理します。
- たとえば `cruise ssh <host> -- install jcode` のようにして、接続先に管理対象プロバイダーをインストールできます。

<a id="installation"></a>
## インストール

<a id="windows-powershell"></a>
### Windows（PowerShell）

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.ps1 | iex"
```

<a id="macos-and-linux-shell-installer"></a>
### macOS と Linux（シェルインストーラー）

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh | sh
```

シェルおよび PowerShell インストーラーは `CARGO_HOME` の設定にかかわらず `cruise` を `~/.local/bin`（Windows では `%USERPROFILE%\.local\bin`）へ配置します。このディレクトリを `PATH` に含めてください。サポート対象の管理インストールでは `cruise update` を使えます。下記の[更新](#updating-cruise)を参照してください。特定のリリースをインストールするには、インストーラー URL の `/releases/latest/` を `/releases/download/v<version>/` に置き換えます（例: `v0.2.15`）。Homebrew でインストールした場合は、以後も Homebrew で管理してください。

<a id="homebrew"></a>
### Homebrew

```sh
brew install smartcrabai/tap/cruise
```

Homebrew 版をアップグレードするには:

```sh
brew upgrade smartcrabai/tap/cruise
```

<a id="updating-cruise"></a>
### Cruise の更新

サポート対象の管理インストールでは、更新前後のバージョンを確認してください。

```sh
cruise --version; cruise update; cruise --version
```

`cruise update` は公式の最新安定版を取得します。インストール済みのバージョンと同じか、それより古い場合は何もしません。ネイティブ更新に対応するリリースターゲットは `aarch64-apple-darwin`、`x86_64-apple-darwin`、`aarch64-unknown-linux-gnu`、`x86_64-unknown-linux-gnu`、`x86_64-pc-windows-msvc` です。

| インストール方法 | 更新方法 |
|---|---|
| `~/.local/bin`（Windows では `%USERPROFILE%\.local\bin`）へのシェルまたは PowerShell インストール、または既存の正規 `$CARGO_HOME/bin` インストール | `cruise update` |
| 正規の `$CARGO_HOME/bin` にあるソースビルド版 | `cruise update`。ローカルのソースビルドやビルド時の変更ではなく、公式ランタイムをインストールします |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action、Docker イメージ、その他の外部インストール | その配布方法に応じた元の更新方法を使います |
| `CRUISE_RUNTIME` が設定されているすべてのインストール | オーバーライドを解除し、管理ランチャーから `cruise update` を実行します |

「管理対象」かどうかは Cargo のインストール台帳ではなく、実行ファイルの場所（既定のインストーラー先 `~/.local/bin`（Windows では `%USERPROFILE%\.local\bin`）または正規の `$CARGO_HOME/bin`）で判定されます。アップデーターが切り替えるのはランタイム選択のみです。固定の `cruise` ランチャー（Windows では `cruise.exe`）はそのまま残り、更新ごとにバージョン付きの `cruise-runtime-managed-…` ランタイムと `.json` サイドカーが追加され、`.cruise-runtime-active` が切り替わります。古いランタイムも保持されます。ネイティブ更新は Cargo を実行せず、Cargo のインストール台帳も変更しません。カスタムのソースビルド版が必要な場合は、ソースから再ビルドしてインストールしてください。

Cruise を更新しても、すでに実行中の WebUI プロセスのコードは置き換わりません。更新後に `cruise webui status` でそのバージョンと現在のランタイムを比較し、`cruise webui restart` で更新済みランタイムを読み込んでください。再起動すると実行中の WebUI 操作がキャンセルされるため、適切なタイミングで行ってください。

<a id="managed-provider-clis"></a>
### 管理対象プロバイダー CLI

ワークフローで必要なプロバイダーだけをインストールしてください。

```sh
cruise install jcode
cruise install claude
cruise install opencode
cruise install --status
cruise install jcode --force
```
