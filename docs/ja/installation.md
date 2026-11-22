**言語:** [English](../en/installation.md) | [日本語](../ja/installation.md) | [简体中文](../zh/installation.md) · [目次](README.md)

<a id="prerequisites"></a>
## 前提条件

- [`gh` CLI](https://cli.github.com/) — GitHub の worktree セッションで自動 PR 作成（既定）、PR に基づくクリーンアップ、または明示的に有効化したワークフロー内マージを使う場合に必要です。GitLab リポジトリでは `glab` を使用します。`create_pr: false` のローカル worktree ワークフローには forge CLI は不要ですが、`--repo` ではリポジトリへのアクセスとクローンのため `gh`/`glab` が必要です。
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) — GitLab リポジトリでは `gh` の代わりに必要です（merge request の作成、クリーンアップ、issue の公開、明示的に有効化したネイティブ同期 squash merge）。forge はホスト名（`github.com` / `gitlab.com`）から判定されます。セルフマネージドホストでは `CRUISE_FORGE=github|gitlab` を設定し、`owner/repo` 形式の `--repo` ロケーターでは `GITLAB_HOST` も設定します。`--repo` は `owner/repo`、ネストされた `group/sub/project`（`CRUISE_FORGE=gitlab` の場合）、または完全な URL を受け付けます。GitLab では `{pr.number}` / `{pr.url}` は merge request を指し、issue 公開時に `@cruise run` コメントを投稿するオプションは使用できません。
- [`jcode` CLI](https://github.com/1jehuang/jcode) — 既定の SDK backend（`sdk: jcode`）で必要です。インストール後に `jcode login <provider>` でサインインしてください。ワークフローで jcode SDK を使う場合にインストールします。
- `cruise ssh` 用に、ローカルの `PATH` 上に OpenSSH クライアント（`ssh`）が必要です。
- Windows では [Git for Windows](https://gitforwindows.org/)（`PATH` 上の `git`）が必要です。
- SSH 接続先には、互換性のある `cruise` と、選択したワークフローに必要な通常の前提条件（必要に応じて `gh` や `jcode` など）が必要です。ワークフローはリモートホストで実行され、認証、設定、セッション、worktree、クローンもそのホストが管理します。
- たとえば `cruise ssh <host> -- install jcode` のようにして、接続先に管理対象 jcode CLI をインストールできます。

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

`cruise update` の後、次の client request で idle daemon は新しい runtime に自動再起動します。daemon が busy の場合は古い version のまま稼働し、CLI client は request を拒否して、idle 時に `cruise daemon restart` を実行するよう案内します。WebUI も再起動までは古い version のままです。`cruise daemon status` で daemon を確認してください。削除された `cruise webui status` や `restart` は使わないでください。

<a id="docker"></a>
### Docker

Docker image は長時間稼働する service container として起動し、credential などの process-level 環境変数は `--env-file` などで container に渡してください。

```sh
docker run -d --name cruise -p 127.0.0.1:8484:8484 --env-file .env cruise:latest
```

WebUI の login URL は `docker logs cruise` に表示されます。URL に含まれるのは 5 分で期限切れになる一度だけ使える nonce で、registry API token ではありません。意図して network に公開する場合を除き、service は loopback（`-p 127.0.0.1:8484:8484`）だけに公開してください。daemon は平文 HTTP で配信されます。同じ service container 内で CLI command を実行します。

```sh
docker exec cruise cruise plan "implement the feature"
```

`docker run --rm --env-file .env cruise:latest plan "implement the feature"` のような one-shot container は foreground では動作します。終了後に container と daemon も停止するため、background の `--plan` や detached work には one-shot container を使わないでください。credential と process-level 環境変数は `--env-file` または `-e` で service container に渡してください。

<a id="managed-provider-clis"></a>
### 管理対象 jcode CLI

Cruise が管理する jcode CLI をインストール、または状態を確認します。

```sh
cruise install jcode
cruise install --status
cruise install jcode --force
```

各インストール時に jcode の最新安定版リリース情報を確認します。選択されたバージョンと一致する正常な管理対象インストールがあれば再利用し、別バージョンなら更新します。`--force` は選択された最新バージョンを再ダウンロードします。

インストールにはリリースメタデータと配布ファイルへのネットワークアクセスが必要で、上流の変更が互換性に影響する場合があります。ダウンロードは配布元が提供するチェックサムで検証しますが、独立した署名ではありません。`cruise install --status` はオフラインで動作し、最新かどうかを主張せず、記録されたインストール済みバージョンを表示します。管理ランタイムの検証も Cruise ビルドの固定バージョンではなく、記録されたインストール済みバージョンを基準にします。
