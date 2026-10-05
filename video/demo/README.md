# 操作デモ版（日本語・71秒）

完成ファイルは [`../out/kikimimi-demo-ja.mp4`](../out/kikimimi-demo-ja.mp4)、サムネイルは [`../out/demo-poster.png`](../out/demo-poster.png)。以前の45秒紹介版は残しています。

## X向け縦型版（日本語・76秒）

完成ファイルは [`../out/kikimimi-demo-vertical-ja.mp4`](../out/kikimimi-demo-vertical-ja.mp4)、サムネイルは [`../out/demo-vertical-poster.png`](../out/demo-vertical-poster.png)。1080×1920 / 30fps / 音声なし。

900×1300の縦長ウィンドウで実アプリとCLIを再録画し、上部1560pxに操作画面、下部に日本語字幕を配置しています。タイトルスライド・説明カード・二段の画面表示はありません。0–64秒は個人/チーム、Models、MCP/Skills、Tools、CLIの操作。64–76秒は実際のStorage & sharingページを開き、S3のセットアップ手順を展開します。

macOSアプリ（開発プレビュー）の提供と、ローカル履歴・任意のクラウド/S3保存は字幕で紹介します。デスクトップのネイティブ操作や実際のクラウド/S3送信は行いません。仕様の根拠は `desktop/README.md` と `docs/src/content/docs/storage-and-sharing.md` です。

ソースは `src/VerticalDemo.tsx` と `src/vertical.css`、録画は `public/demo-portrait/`。デモサーバー起動後、`DEMO_PORTRAIT=1` で同じ録画スクリプトを実行できます。

```sh
npm --prefix video run demo:vertical:record
npm --prefix video run demo:vertical
npm --prefix video run demo:vertical:poster
```

## 3つのユースケース

| 時間 | 実際に行う操作 | 発見 |
| --- | --- | --- |
| 0–3秒 | 導入 | 3人・7日分・21セッションのテストデータ |
| 3–13秒 | Personal → Studio Demo、Membersへ移動 | 個人からチームへ分析範囲を切り替える |
| 13–27秒 | Modelsへ移動、スクロール、In subagentsで並べ替え | Opusの28リクエストがすべてサブエージェント由来 |
| 27–41秒 | MCP → Skills、未使用行へ移動 | notionは0回、code-reviewは21回、release-notesは未使用 |
| 41–50秒 | Tools → Failuresで並べ替え | Playwrightは42回中6失敗、p95は30秒 |
| 50–67秒 | `kikimimi query unused-mcp` → `kikimimi query tools` | CLIでも同じ0回／42回・6失敗を確認 |
| 67–71秒 | 終了 | 公式サイトへの案内 |

日本語テロップ中心・音声なし。`src/DemoVideo.tsx` が構成と説明文、`src/demo.css` が外枠です。操作画面は **Playwrightで録画した実映像**をRemotionのOffthreadVideoで読み込みます。カーソルとクリック波紋は録画用に重ね、後半の注目箇所は映像を拡大しています。

## 実アプリ・テスト環境の境界

- アプリは `web/` をビルドした本物のReact UIです。
- Overview・Models・Tools・Skills・MCPなどは、最新ソースからビルドした **Rustの実APIとDuckDB**で集計します。固定の集計値をUIに返すモックではありません。
- 撮影用サーバーが、個人用／チーム用のParquetを読む2つの `kikimimi web --read-only` を起動します。収集エージェントは起動しません。
- 認証・ワークスペース名簿はローカルの撮影用アダプターです。実クラウドのログインや共有操作は行いません。Membersの集計は `crates/cloud/src/web_query_sql.rs` の `MEMBERS_SQL` を同じテストParquetに適用します。
- CLI画面は撮影用のブラウザー端末です。入力した許可済みコマンドで **実際のkikimimiバイナリを起動**し、stdoutを表示しています。コマンド出力は手書きではありません。任意のシェルコマンドを実行する機能はありません。
- すべて架空のテストデータです。モデル名はシナリオのラベル、コストはテスト値であり、価格・性能比較ではありません。サブエージェント分析はClaude Codeで取得できるメタデータを使うケースです。
- ローカルサーバーは `127.0.0.1:5186` のみにバインドします。本番アカウント、既存の収集データ、Claude/Codex設定に触れず、クラウドへの送信も行いません。

## テストデータ

`scenario.json` に3人のメンバー・モデル・設定済み拡張を定義しています。`fixtures/events.jsonl` は980件の `kikimimi.v1` 合成イベントです。`seed.py` はスキーマの型をRustのEvent定義から読み、専用の `.state/` にParquetと設定を生成します。

通常は撮影日の直近7日として生成し、実アプリの日付フィルターで表示されるようにします。`DEMO_DATE=YYYY-MM-DD` で固定日付も指定可能ですが、古い日付では画面の直近14日フィルターから外れます。

期待する結果は `verify.mjs` で検証します。個人／チームのツール呼び出し総数（119／357）、サブエージェントのモデル帰属、未使用項目、失敗数、UIとCLIの整合性を確認し、実API応答と実CLI出力を `public/demo/evidence/` に保存します。

## 既存の録画から再レンダリング

```sh
npm --prefix video ci
npm --prefix video run check
npm --prefix video run demo:render
npm --prefix video run demo:poster
```

Chromeを明示する場合は各レンダリングコマンドに `-- --browser-executable=/usr/bin/google-chrome --concurrency=2` を追加してください。

## データから操作を再録画

Node.js・Rust・Python 3・DuckDB CLI・FFmpeg/ffprobe・Chromeが必要です。CLIの罫線を揃えるため、録画環境にはNoto Sans MonoまたはLiberation Monoも用意してください。リポジトリルートで実行します。

```sh
npm --prefix web ci
npm --prefix web run build
cargo build -p kikimimi --bin kikimimi
npm --prefix video ci
npm --prefix video run demo:seed
npm --prefix video run demo:serve
```

サーバーを起動したまま、別のターミナルで実行します。

```sh
npm exec --prefix video -- playwright install ffmpeg
npm --prefix video run demo:verify
CHROME_PATH=/usr/bin/google-chrome npm --prefix video run demo:record
npm --prefix video run demo:render
npm --prefix video run demo:poster
```

この制作環境（Ubuntu 26.04）ではPlaywrightのFFmpegプラットフォーム判定に互換指定が必要でした。該当環境では、FFmpegのインストールと録画コマンドの両方に次を付けます。

```sh
PLAYWRIGHT_HOST_PLATFORM_OVERRIDE=ubuntu24.04-x64 \
PLAYWRIGHT_BROWSERS_PATH="$PWD/video/demo/.state/browsers" \
npm exec --prefix video -- playwright install ffmpeg

PLAYWRIGHT_HOST_PLATFORM_OVERRIDE=ubuntu24.04-x64 \
PLAYWRIGHT_BROWSERS_PATH="$PWD/video/demo/.state/browsers" \
CHROME_PATH=/usr/bin/google-chrome npm --prefix video run demo:record
```

録画時の操作ログは `public/demo/recording.json`。最終クリップと検証結果はGitで管理し、元のWebM・一時Parquet・プロセス設定は管理しません。サーバーはCtrl+Cで終了すると、起動した読み取り専用ビューアーも停止します。

一部分だけ撮り直す場合は `DEMO_CLIPS=cli` のようにクリップ名を指定できます（複数はカンマ区切り）。既存の操作ログを保持して、指定クリップだけ更新します。撮り直し後は完成MP4も再レンダリングしてください。

参考: [Playwrightの動画録画](https://playwright.dev/docs/videos)、[Remotion OffthreadVideo](https://www.remotion.dev/docs/offthreadvideo)。
