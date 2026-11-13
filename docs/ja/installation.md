**言語:** [English](../en/installation.md) | [日本語](../ja/installation.md) | [简体中文](../zh/installation.md) · [目次](README.md)

<a id="prerequisites"></a>
## 前提条件

- [`gh` CLI](https://cli.github.com/) — GitHub の worktree セッションで自動 PR 作成（既定）、PR に基づくクリーンアップ、または明示的に有効化したワークフロー内マージを使う場合に必要です。GitLab リポジトリでは `glab` を使用します。`create_pr: false` のローカル worktree ワークフローには forge CLI は不要ですが、`--repo` ではリポジトリへのアクセスとクローンのため `gh`/`glab` が必要です。
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) — GitLab リポジトリでは `gh` の代わりに必要です（merge request の作成、クリーンアップ、issue の公開、明示的に有効化したネイティブ同期 squash merge）。forge はホスト名（`github.com` / `gitlab.com`）から判定されます。セルフマネージドホストでは `CRUISE_FORGE=github|gitlab` を設定し、`owner/repo` 形式の `--repo` ロケーターでは `GITLAB_HOST` も設定します。`--repo` は `owner/repo`、ネストされた `group/sub/project`（`CRUISE_FORGE=gitlab` の場合）、または完全な URL を受け付けます。GitLab では `{pr.number}` / `{pr.url}` は merge request を指し、issue 公開時に `@cruise run` コメントを投稿するオプションは使用できません。
- [`jcode` CLI](https://github.com/1jehuang/jcode) v0.88.0 以降 — 既定の SDK バックエンド（`sdk: jcode`）で必要です。インストール後に `jcode login <provider>` でサインインしてください。SDK 実行では、別のワークフロー SDK の下にある SDK 指定配列項目も含め、実行または計画の有効な候補が jcode を使う場合にインストールしてください。
- [`claude` CLI](https://code.claude.com/docs/en/quickstart) — 実行または計画の有効な候補が claude を使う場合に必要です（SDK 指定配列項目を含む）。Cruise はプロセス内で claude を実行します。2.1.250 で検証済みです。モデル名の `:effort` 接尾辞は `claude --effort` に対応するため、このフラグのない CLI ではステップが `unknown option '--effort'` で失敗します（恒久的なエラーであり、再試行されません）。認証は CLI 独自のもので、`jcode login` とは無関係です。
- V2 API 対応の `opencode` CLI — 実行または計画の有効な候補が opencode を使う場合に必要です（SDK 指定配列項目を含む）。Cruise はローカルで起動・停止し、既存の OpenCode プロバイダー設定と認証を使用します。V1 はサポートされません。
- `cruise ssh` 用に、ローカルの `PATH` 上に OpenSSH クライアント（`ssh`）が必要です。
- Windows では [Git for Windows](https://gitforwindows.org/)（`PATH` 上の `git`）が必要です。`command:` ステップは `cmd.exe /C` で実行されるため、POSIX シェル構文ではなく cmd.exe 構文を使うか、明示的に `powershell -Command ...` / `bash -c ...` を呼び出してください。デスクトップ通知と通知音は利用できませんが、設定済みの webhook は送信されます。`cruise ssh` には Windows OpenSSH クライアントが必要です。herdr への報告は herdr 環境変数で提供される範囲に限られます。
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

または、ダウンロードが成功した場合にのみ実行するよう、インストーラーをファイルに保存します。

```sh
curl --proto '=https' --tlsv1.2 -fLsS --output cruise-installer.sh \
  https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh \
  && sh ./cruise-installer.sh
```

シェルおよび PowerShell インストーラーは `cruise` を `$CARGO_HOME/bin`（通常は `~/.cargo/bin`。Windows では `CARGO_HOME` 未設定時に `%USERPROFILE%\.cargo\bin`）へ配置します。このディレクトリを `PATH` に含めてください。サポート対象の管理インストールでは `cruise update` を使えます。下記の[更新](#updating-cruise)を参照してください。特定のリリースをインストールするには、インストーラー URL の `/releases/latest/` を `/releases/download/v<version>/` に置き換えます（例: `v0.2.15`）。Homebrew でインストールした場合は、以後も Homebrew で管理してください。

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
| 正規の `$CARGO_HOME/bin` にあるシェルまたは PowerShell インストーラー版 | `cruise update` |
| 正規の `$CARGO_HOME/bin` にあるソースビルド版 | `cruise update`。ローカルのソースビルドやビルド時の変更ではなく、公式ランタイムをインストールします |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action、Docker イメージ、その他の外部インストール | その配布方法に応じた元の更新方法を使います |
| `CRUISE_RUNTIME` が設定されているすべてのインストール | オーバーライドを解除し、管理ランチャーから `cruise update` を実行します |

「管理対象」かどうかは Cargo のインストール台帳ではなく、正規の `$CARGO_HOME/bin` にあるかで判定されます。アップデーターが切り替えるのはランタイム選択のみです。固定の `cruise` ランチャー（Windows では `cruise.exe`）はそのまま残り、更新ごとにバージョン付きの `cruise-runtime-managed-…` ランタイムと `.json` サイドカーが追加され、`.cruise-runtime-active` が切り替わります。古いランタイムも保持されます。ネイティブ更新は Cargo を実行せず、Cargo のインストール台帳も変更しません。カスタムのソースビルド版が必要な場合は、ソースから再ビルドしてインストールしてください。

アップデーターを含まない公開済みバージョン（v0.2.16 など）は、自分自身を更新できません。アップデーター対応ビルドから v0.2.16 を選ぶことはできますが、その公開済みランタイムから次のネイティブ更新は行えません。プラットフォーム用インストーラーでアップデーター対応リリースをインストールするか、そのソースバージョンを再ビルド・インストールして初期状態を整えてください。以後 `cruise update` を使うのは、インストール済みランタイムにアップデーターが含まれる場合に限ります。

ダウンロードしたリリースは HTTPS 経由で取得され、リリースマニフェストに記載された SHA-256 チェックサムと照合されます。チェックサムも同じリリース発行元が配布するため、独立したデジタル署名ではありません。

`cruise update --status` は全インストールの記録済みトランザクションを順序付きで表示します。読み取り専用で、ネットワーク接続、更新ロックの取得、状態の修復は行わず、現在有効なランタイムの証明にもなりません。トランザクションファイルは `$XDG_STATE_HOME/cruise/installations/<installation-id>/update-transaction.json`（既定は `~/.local/state/cruise/installations/<installation-id>/update-transaction.json`。Windows では `XDG_STATE_HOME` 未設定時に `%LOCALAPPDATA%\cruise\state\installations\<installation-id>\update-transaction.json`）です。失敗したトランザクションにはフェーズとエラーが保持されます。`cruise update` を再実行すると、新たなダウンロードを始める前に中断済みトランザクションを調整します。有効化前の失敗では以前の選択が維持されます。有効化後のランチャープローブが失敗した場合は、以前の選択に戻されます。

更新が適用済みでも、トランザクション完了やクリーンアップの警告が出た場合、新しいランタイムはすでに有効であり、ロールバックされていません。`cruise update --status` を確認し、その後 `cruise update` を再試行して、記録済みトランザクションを回復または整理してください。ステータスのフェーズはメタデータであり、有効化の証明ではありません。実行中のバージョンは `cruise --version` で確認します。

Cruise を更新しても、すでに実行中の WebUI プロセスのコードは置き換わりません。更新後に `cruise webui status` でそのバージョンと現在のランタイムを比較し、`cruise webui restart` で更新済みランタイムを読み込んでください。再起動すると実行中の WebUI 操作がキャンセルされるため、適切なタイミングで行ってください。

バージョン付きランタイムは `$CARGO_HOME/bin`（Windows の既定では `%USERPROFILE%\.cargo\bin`）内に共存できます。インストール済みの特定バージョンを固定するには、そのバージョン付き実行ファイルに対して `--version` を実行し、隣接する `.json` サイドカーが現在の固定ランチャー／ランタイムのベースラインに属することを確認したうえで、`.cruise-runtime-active` にランタイムの正確なベース名を設定します。他のランタイムファイルは削除しないでください。ベースラインは固定ランチャー／ランタイムのサイズと mtime の組み合わせです。再インストールでそれが変わるとオーバーレイは無効になりますが、サイズと mtime が同じ置き換えは検出されません。古いポインターや古いランチャーの固定では手動での整理が必要な場合があります。ポインターを退避するには、まず固定の兄弟ランタイムが直接起動することを確認し、その後に限ってポインターファイルを保存・移動してください。

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

上記コマンドでは、最初の `--version` が成功してからポインターを移動してください。これらのコマンドはポインターファイルを保存して移動するだけです。ランタイムやサイドカーは決して削除しないでください。保存先または退避先がすでに存在する場合は、未使用の名前を選びます。

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

このビルドが固定するバージョンは jcode **0.99.1**、claude **2.1.250**、opencode **2.1.3** です。ダウンロードは Cruise に固定されたダイジェストと照合され、展開したバイナリのバージョンを公開前に検証します。ベンダーのインストーラーや npm CLI は不要で、シェル設定も変更しません。管理対象ファイルは `$XDG_DATA_HOME/cruise/providers/<provider>/`（通常は `~/.local/share/cruise/providers/`）に置かれます。各インストールには異なるバージョン接頭辞付きディレクトリを使い、有効な相対パスを `install.json` に記録します。並行して使っている読み手が解決済みパスを引き続き使えるよう、以前のディレクトリも保持されます。利用可否はプラットフォームによって異なります。認証は各プロバイダーの責任です。

jcode を Cruise 経由でのみインストールした場合は、Cruise のデータディレクトリにある `providers/jcode/install.json` から有効な `relative_path` を確認し、そのバイナリに `login <provider>` または `auth status` を指定して実行します。Windows では `XDG_DATA_HOME` が未設定で `LOCALAPPDATA` が設定されている場合、データディレクトリの既定は `%LOCALAPPDATA%\cruise\data` です。`LOCALAPPDATA` がない場合はホームディレクトリへのフォールバックが適用されます。空白を含むパスは引用符で囲んでください。

解決順序は、空でないプロセス環境変数のオーバーライド、現在の固定バージョンに一致する有効な管理対象インストール、既存の PATH 検索です。オーバーライドは `CRUISE_JCODE_CLI_PATH`、`CRUISE_CLAUDE_CLI_PATH`、`CRUISE_OPENCODE_CLI_PATH` です。明示したオーバーライドが無効な場合はエラーになり、フォールバックしません。これらはワークフローの `env:` 設定ではなく、プロセス環境変数です。管理対象インストールが欠落、古い、または破損している場合は PATH にフォールバックします。実行中のワークフロー、計画、TUI、WebUI がプロバイダーをダウンロードまたはインストールすることはありません。

`--status` は読み取り専用で、ネットワークにアクセスせず、プロバイダーバイナリを実行せず、ファイルも作成しません。必要な場合は `--force` で再インストールしてください。Cruise の更新で固定バージョンが変わったら `cruise install <provider>` を再実行してください。`cruise update` が更新するのは Cruise 自体であり、プロバイダーではありません。管理対象の claude と opencode については、それぞれの環境変数がすでに設定されていない場合に限り、Cruise が自動更新機能を無効にします。PATH やオーバーライド経由のインストールでは、既存の動作が維持されます。GitHub Action は引き続き独自に jcode のインストールとバージョン入力を管理します。
