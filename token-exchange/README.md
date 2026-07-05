# cruise-token-exchange

`cruise-agent` GitHub App 用の OIDC トークン交換サービス。GitHub Actions ワークフローが自身の
OIDC ID トークン（`id-token: write` パーミッションで発行されるもの）をこの Cloudflare Worker に
渡すと、Worker が検証の上でその呼び出し元リポジトリに**限定**した GitHub App installation access
token を返す。

これにより cruise を CI で動かす利用者は、GitHub App の秘密鍵を自分のリポジトリ/Organization の
Secrets に置くことなく、`cruise-agent[bot]` としてコミット・PR・コメントを行える。方式は
[anthropics/claude-code-action](https://github.com/anthropics/claude-code-action) の
`api.anthropic.com/api/github/github-app-token-exchange` と同じ考え方（呼び出し元の
Actions OIDC トークンをサーバー側で検証し、リポジトリスコープの installation token を発行する）
に倣っている。

## 目次

- [仕組み](#仕組み)
- [API 仕様](#api-仕様)
- [セットアップ・デプロイ](#セットアップデプロイ)
- [ローカル開発・テスト](#ローカル開発テスト)
- [セルフホスト手順](#セルフホスト手順)
- [セキュリティモデル](#セキュリティモデル)
- [既知の制限・残課題](#既知の制限残課題)

## 仕組み

1. GitHub Actions ワークフローが `id-token: write` パーミッションで、
   audience `cruise-agent-token-exchange` を指定した OIDC ID トークンを取得する。
2. ワークフローがそのトークンを `Authorization: Bearer <token>` としてこの Worker の
   `POST /token` に送る（リクエストボディは不要）。
3. Worker は以下を行う。
   - `https://token.actions.githubusercontent.com/.well-known/jwks` の公開鍵で JWT 署名を検証し、
     `iss`（issuer）・`aud`（audience）・`exp`/`nbf` を厳密にチェックする。
   - 検証済みトークンの `repository` クレーム（`owner/repo` 形式）から対象リポジトリを取得する
     （リクエスト側が指定するリポジトリは一切信用しない）。
   - GitHub App としての短命 JWT を生成し、`GET /repos/{owner}/{repo}/installation` で
     そのリポジトリへの installation を解決する（未インストールなら 404 を返す）。
   - `POST /app/installations/{id}/access_tokens` で、**そのリポジトリのみ**・
     **contents/pull_requests/issues の write のみ**に絞った installation access token を発行する。
4. ワークフローは返ってきた `token` を `actions/checkout` の `token` や `gh` CLI の認証に使い、
   `cruise-agent[bot]` として動作する。

## API 仕様

action 側の実装と共有する確定仕様。変更する場合は両側を同時に更新すること。

### `POST /token`

- リクエスト: `Authorization: Bearer <GitHub Actions OIDC JWT>`（ボディなし）
- OIDC audience: **`cruise-agent-token-exchange`**（固定値）
- 成功 200:
  ```json
  {
    "token": "ghs_...",
    "expires_at": "2026-07-04T12:00:00Z",
    "installation_id": 12345678
  }
  ```
- エラー（JSON `{"error":"<code>","message":"..."}`)）:

  | status | error code | 発生条件 |
  | --- | --- | --- |
  | 401 | `invalid_oidc` | 署名/`iss`/`aud`/`exp` 検証失敗、`Authorization` ヘッダ欠落、`repository` クレームの形式不正 |
  | 404 | `not_installed` | cruise-agent App が対象リポジトリに未インストール。message に `https://github.com/apps/cruise-agent/installations/new` を含む |
  | 405 | `method_not_allowed` | POST 以外のメソッドで `/token` にアクセス |
  | 502 | `github_error` | GitHub API 呼び出し失敗、または `GITHUB_APP_ID`/`GITHUB_APP_PRIVATE_KEY` の設定不備 |

### `GET /healthz`

認証不要。常に 200 `{"ok":true}` を返す。

## セットアップ・デプロイ

前提: [cruise-agent GitHub App](https://github.com/apps/cruise-agent) が作成済みで、
以下のリポジトリ権限を持つこと。

- Contents: Read & write
- Pull requests: Read & write
- Issues: Read & write
- Metadata: Read-only（自動付与）

```bash
cd token-exchange
npm install
```

### 1. GitHub App の秘密鍵を PKCS8 に変換する

GitHub の App 設定画面からダウンロードできる秘密鍵は **PKCS1**
（`-----BEGIN RSA PRIVATE KEY-----`）形式だが、この Worker は `jose` の `importPKCS8` を使うため
**PKCS8**（`-----BEGIN PRIVATE KEY-----`）形式が必要。変換してから登録すること。

```bash
openssl pkcs8 -topk8 -inform PEM -in <ダウンロードした鍵>.pem -out key-pkcs8.pem -nocrypt
```

PKCS1 のまま登録した場合、Worker はリクエスト時に明確なエラーメッセージ
（`GITHUB_APP_PRIVATE_KEY is PKCS1 ...`）とともに `502 github_error` を返す
（`src/github-app.ts` の `createAppJwt` 参照）。

### 2. Secrets を登録する

```bash
wrangler secret put GITHUB_APP_ID
# プロンプトで App ID (数値) を入力

wrangler secret put GITHUB_APP_PRIVATE_KEY < key-pkcs8.pem
```

変換した `key-pkcs8.pem` はローカルに残さないこと（登録後は削除する）。

### 3. デプロイ

```bash
npx wrangler deploy
```

デプロイ後の動作確認:

```bash
curl https://<your-worker>.workers.dev/healthz
# {"ok":true}
```

## 本番運用（smartcrab公式インスタンス）

公式インスタンス `https://cruise-token-exchange.smartcrab.ai` は上記の手動手順ではなく、次の分業で運用している:

- **デプロイ**: `main` への push で `token-exchange/` に変更があると
  [`.github/workflows/deploy-token-exchange.yml`](../.github/workflows/deploy-token-exchange.yml)
  が typecheck + テスト後に `wrangler deploy` を実行し、Worker secrets
  （`GITHUB_APP_ID` / `GITHUB_APP_PRIVATE_KEY`）も GitHub Actions secrets
  （`CRUISE_APP_ID` / `CRUISE_APP_PRIVATE_KEY`）から同期する。手動デプロイ・手動
  `wrangler secret put` は不要。必要な GitHub Actions secrets:
  `CLOUDFLARE_API_TOKEN`（"Edit Cloudflare Workers" テンプレート）、
  `CLOUDFLARE_ACCOUNT_ID`、`CRUISE_APP_ID`、`CRUISE_APP_PRIVATE_KEY`（PKCS8）
- **カスタムドメイン**: `cruise-token-exchange.smartcrab.ai` の紐付けは terraforms
  リポジトリ（`environments/production/cloudflare/workers.auto.tfvars` の
  `workers_custom_domains`）で Terraform 管理。`wrangler.jsonc` には routes /
  custom domain を書かない（デプロイごとに Terraform 管理の紐付けと競合するため）
- **workers.dev URL**: `wrangler.jsonc` の `workers_dev: false` で無効化しており、
  公開エンドポイントはカスタムドメインのみ

### 4. 呼び出し側ワークフロー例

```yaml
permissions:
  id-token: write
  contents: read

jobs:
  cruise:
    runs-on: ubuntu-latest
    steps:
      - name: Request OIDC token
        id: oidc
        run: |
          TOKEN=$(curl -sSL -H "Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
            "$ACTIONS_ID_TOKEN_REQUEST_URL&audience=cruise-agent-token-exchange" | jq -r '.value')
          echo "::add-mask::$TOKEN"
          echo "token=$TOKEN" >> "$GITHUB_OUTPUT"

      - name: Exchange for installation token
        id: exchange
        run: |
          RESPONSE=$(curl -sSL -w '\n%{http_code}' -X POST \
            -H "Authorization: Bearer ${{ steps.oidc.outputs.token }}" \
            https://<your-worker>.workers.dev/token)
          STATUS=$(echo "$RESPONSE" | tail -n1)
          BODY=$(echo "$RESPONSE" | sed '$d')
          if [ "$STATUS" != "200" ]; then
            echo "token exchange failed: $BODY" >&2
            exit 1
          fi
          INSTALL_TOKEN=$(echo "$BODY" | jq -r '.token')
          echo "::add-mask::$INSTALL_TOKEN"
          echo "token=$INSTALL_TOKEN" >> "$GITHUB_OUTPUT"

      - uses: actions/checkout@v4
        with:
          token: ${{ steps.exchange.outputs.token }}
```

## ローカル開発・テスト

```bash
npm run typecheck        # tsc --noEmit
npm test                 # vitest run（JWT検証・GitHub API呼び出しはモック、実ネットワーク不要）
npm run deploy:dry-run   # wrangler deploy --dry-run
```

ローカルで `wrangler dev` を使う場合は `.dev.vars.example` を `.dev.vars`（gitignore 済み）に
コピーし、`GITHUB_APP_ID` / `GITHUB_APP_PRIVATE_KEY` を設定する。`wrangler types` を実行すると
`wrangler.jsonc` と `.dev.vars` のキーから `worker-configuration.d.ts`（`Env` 型、gitignore済み）
が再生成される。`wrangler.jsonc` を変更した場合は再実行すること。

テストは実際の Cloudflare ランタイム（miniflare/workerd）ではなく素の vitest 上で動く。
`src/index.ts` の `createHandler(deps)` が JWT 検証用の `getKey`（`jose` の
`JWTVerifyGetKey`）と GitHub API 呼び出し用の `fetchGithub` を依存注入で受け取る設計になっており、
テストでは:

- `getKey` → `src/test-helpers.ts` の `createTestOidcIssuer()` が生成する使い捨て RSA 鍵ペア +
  `createLocalJWKSet`
- `fetchGithub` → `vitest` の `vi.fn` によるスタブ

に差し替えることで、本物の GitHub / GitHub Actions OIDC エンドポイントに一切アクセスせず
成功系・異常系（`invalid_oidc` / `not_installed` / 405 / `repository` クレーム形式不正 /
GitHub API エラー / 秘密鍵形式不正）を検証している。

## セルフホスト手順

第三者が自分の GitHub App + 自分の Cloudflare アカウントでこのサービスを運用する場合:

1. 自分の GitHub App を作成し、Contents/Pull requests/Issues を Read & write に設定して
   対象リポジトリにインストールする。
2. このディレクトリを fork/コピーし、`wrangler.jsonc` の `name` を自分の Worker 名に変更する
   （`cruise-token-exchange` のままだと自分の Cloudflare アカウント内でのみ衝突しなければ動くが、
   デプロイ URL が変わる点に注意）。
3. `wrangler secret put GITHUB_APP_ID` / `wrangler secret put GITHUB_APP_PRIVATE_KEY` で
   自分の App の資格情報を登録して `wrangler deploy`。
4. 呼び出し側の GitHub Actions ワークフロー（cruise の action、または独自の action）から、
   自分の Worker の URL に対して OIDC トークンを送るよう設定する。
5. OIDC audience（`src/oidc.ts` の `OIDC_AUDIENCE = "cruise-agent-token-exchange"`）は
   Worker とワークフローの両方で一致している必要がある固定値。別の audience を使いたい場合は
   `src/oidc.ts` を変更した上で、ワークフロー側の `audience=` パラメータも同じ値に揃えること。
   デフォルト値のまま複数の第三者が別々の GitHub App/Worker を運用しても、audience が同じでも
   `repository` クレームで対象リポジトリが縛られ、かつ各自の Worker は自分の App の installation
   しか解決できないため、他人のリポジトリのトークンが発行されることはない。

## セキュリティモデル

- **OIDC 検証は `jose` に一任**: `createRemoteJWKSet`（GitHub の JWKS を組み込みキャッシュ付きで
  取得）+ `jwtVerify` で署名・`iss`・`aud`・`alg`（RS256 固定）・`exp`/`nbf` を検証する。
  検証に失敗した場合は理由を問わず `401 invalid_oidc` として扱い、詳細な失敗理由はレスポンスの
  `message` に含めるが、トークン自体やクレーム全体はログに出さない。
- **対象リポジトリはトークンの `repository` クレームのみが決める**: リクエストボディやクエリ
  パラメータでリポジトリを指定させる経路は存在しない。これにより、あるリポジトリの Actions
  ワークフローが別リポジトリ用のトークンを要求することはできない。
- **App としての認証は短命 JWT**: `iat: now-60`（時計ずれ許容）、`exp: now+540`
  （GitHub の上限10分に対し余裕を持たせて9分）。秘密鍵は `importPKCS8` でインポートし、
  PKCS1 形式が渡された場合は明確なエラーメッセージで弾く。
- **installation token は最小権限で発行**: `repositories: [<name>]` で対象リポジトリのみに絞り、
  `permissions` も `contents`/`pull_requests`/`issues` の `write` のみを明示指定する。
  App 自体がそれ以上の権限を持っていても、発行するトークンはこれ以上の権限を持たない。
- **ログにトークン・秘密鍵を出さない**: 例外処理では捕捉した Error のメッセージのみを
  クライアントに返し、内部の予期しない例外は `Unexpected error while exchanging the token`
  という定型文に丸めて詳細を隠す（スタックトレースなどの意図しない情報漏洩を防ぐため）。
- **CORS ヘッダは付与しない**: このサービスはブラウザから直接呼ばれることを想定しない
  server-to-server 専用エンドポイントであり、`Access-Control-Allow-Origin` 等は一切返さない。

## 既知の制限・残課題

- **installation 解決のキャッシュなし**: リクエストごとに GitHub App JWT の生成と
  `GET /repos/{owner}/{repo}/installation` 呼び出しを行う。頻度が高くなる場合は KV 等で
  installation ID を短時間キャッシュする余地があるが、まずは正しさ優先でキャッシュなしとした。
- **レート制限・乱用対策は未実装**: Cloudflare 側のレート制限ルールや、同一リポジトリからの
  異常な発行頻度の検知などは今回のスコープ外。
- **cruise 本体の action.yml / action/ 側の統合は別作業**: 本 PR はこの Worker 単体の実装のみで、
  `action.yml` や `action/scripts/*` を GitHub App 方式に切り替える変更は含んでいない
  （別エージェント/別 PR で行う想定）。
- **監視・アラート未設定**: `observability.enabled = true` で Workers Logs は有効化しているが、
  エラー率アラート等の運用面は未整備。
