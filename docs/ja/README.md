**言語:** [English](../en/README.md) | [日本語](../ja/README.md) | [简体中文](../zh/README.md)

# cruise

<p align="center">
  <img src="../icon.png" alt="cruise logo" width="160" height="160">
</p>

Cruise は YAML ベースの AI ワークフローオーケストレーターです。

Cruise は宣言型ワークフローに沿って計画 → 承認 → テスト作成 → 実装 → テスト → レビュー → 任意で PR 作成と after-PR のような処理を、半決定的に自動で進めます。ステップ間の変数の受け渡し、条件分岐、ループ制御が可能です。Jcodeをベースとしており、リソース消費もかなり小さくなっています。Claude・OpenCode の SDK バックエンドと任意の CLI コマンド（`codex exec` 等）にも対応します。

## ドキュメント

各言語のドキュメントは同じファイル名で構成されています。ガイドを更新するときは、将来の変更も含めて英語・日本語・簡体字中国語の 3 つすべてを更新してください。

- [インストールと前提条件](installation.md) — OS ごとのインストール方法、更新、管理対象 provider CLI。
- [使い方](usage.md) — WebUI、CLI 使用例、herdr、TUI、ワークフローパッケージ。
- [CLI リファレンス](cli-reference.md) — コマンド、引数、オプション、言語設定、通知。
- [セッション管理](sessions.md) — ライフサイクル、workspace モード、分離、復旧、並行実行。
- [設定](configuration.md) — 設定の解決、既定値、設定キー、プロンプト言語。
- [SDK](sdk.md) — バックエンド、認証、アクセス許可、ツール制限。
- [ステップ](steps.md) — ステップの種類、after-PR 自動化、変数。
- [ワークフロー制御](workflow-control.md) — フロー制御、フラグ、条件、グループ、ワークフロー合成。
- [ワークフロー例](workflow-examples.md) — 全開発フローとシンプルな自動 commit フロー。
- [GitHub Actions](github-actions.md) — Action のセットアップ、コマンド、provider、セキュリティ、トラブルシューティング。
- [Telemetry](telemetry.md) — OpenTelemetry の設定、export 内容、プライバシー上の注意。

<a id="development-build"></a>

## 開発ビルド

ランチャーをローカルでテストするには、両方のバイナリをビルドしてください。`cruise` ランチャーは隣接する `cruise-runtime` を起動するため、両方が必要です。

```sh
cargo build --bin cruise --bin cruise-runtime
```

<a id="support"></a>

## サポート

Cruise が役に立った場合は、開発と保守を支援するため[smartcrabai へのスポンサー](https://github.com/sponsors/smartcrabai)をご検討ください。
