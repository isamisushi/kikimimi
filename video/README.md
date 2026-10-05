# kikimimi 紹介動画 — X / 日本語

最新版は [横型デモ版（76秒・1920×1080）](out/kikimimi-demo-ja.mp4)。実画面を中心に、下部へ短い日本語字幕を表示し、注目箇所はズームします。macOSアプリ（開発プレビュー）とローカル・クラウド・自分のS3も紹介しています。`npm --prefix video run demo:render` で再生成できます。

[縦型版（76秒・1080×1920）](out/kikimimi-demo-vertical-ja.mp4)も残しています。[テストデータ・再録画手順](demo/README.md)を参照してください。

Remotionで編集・再生成できる45秒の紹介動画です。1920×1080、30fps、H.264 / yuv420p のMP4。音声なしで内容が伝わるテロップ構成で、BGM・ナレーションは含めていません。

## プレビューと書き出し

```sh
npm --prefix video ci
npm --prefix video run studio
npm --prefix video run check
npm --prefix video run render
npm --prefix video run poster
```

出力は `video/out/kikimimi-ja.mp4` と `video/out/poster.png`。この2つの完成ファイルはGitで管理し、動画を編集した際は再生成して一緒にコミットします。確認用の中間画像と依存パッケージはGit管理対象外です。

Chromeを自動ダウンロードできない環境では、インストール済みのChromeを指定できます。

```sh
npm --prefix video run render -- --browser-executable=/usr/bin/google-chrome --concurrency=2
```

## 構成

| 時間 | 内容 |
| --- | --- |
| 0–4秒 | AIエージェント、何を、どれだけ使ってる？ |
| 4–10秒 | 個人からチームへ：ワークスペース切り替えとメンバー別利用状況 |
| 10–16秒 | Models：モデル別のトークン推移、コスト・effortの分析 |
| 16–21秒 | Skills：呼び出し回数・失敗数・利用セッション |
| 21–26秒 | MCP：サーバー別の利用状況と未使用MCP |
| 26–31秒 | Subagent models：モデル別のサブエージェント使用割合・トークン |
| 31–35秒 | Tools：呼び出し回数・失敗数・所要時間 |
| 35–40秒 | ローカル保存、収集しない内容、任意のクラウド送信 |
| 40–45秒 | ロゴ、開始コマンド、kikimimi.dev |

文言とアニメーションは `src/Video.tsx`、スタイルは `src/style.css`、タイムライン・fpsは `src/timeline.ts`、画面サイズは `src/Root.tsx` で変更します。アニメーションはフレーム数から算出し、再生時刻や乱数に依存しません。

## アプリ画面の再撮影

`public/screenshots/` はこのリポジトリの実際のReact UIを、既存のモックAPIの合成データで撮影したものです。本番の利用データやアカウントは使用していません。分析画面は `/web/me` のローカル表示を有効にし、ワークスペース紹介はクラウド表示で実際のセレクターを操作してPersonalからAcme Incに切り替えます。任意機能のサブスクリプション使用量パネルは無効にしています。UI本体は変更していません。

モデル分析は現在のModelsページの説明に合わせ、Claude CodeのAPIリクエスト記録に基づく旨を表示しています。サブエージェントのモデル分析は同ページの `By model and effort` / `In subagents` 列を使用。チームのメンバー別利用状況は管理者・オーナー向けであることを動画にも明記しています。

開発モックで未実装のAPIは撮影スクリプトで補っています。`/web/q/members` はモックのチーム名簿に架空の使用量を割り当てます。`/web/q/skills` は既存の `unused-skills` のデモ行から呼び出し数・セッション数を転記し、失敗数は不明のままにしています。

別ターミナルでデモサーバーを起動します。

```sh
npm --prefix web ci
npm --prefix web run dev
```

続いて撮影します。撮影スクリプトは `localhost:5173` に固定され、デモ用ログインのみを利用します。

```sh
# 初回のみ、Chromeがない場合
npm exec --prefix video -- playwright install chromium
npm --prefix video run capture

# インストール済みChromeを使用する場合（リポジトリルートから）
CHROME_PATH=/usr/bin/google-chrome npm --prefix video run capture
```

キャプチャの数値はモック、日付は撮影時点の値です。チェックイン済みPNGを使う通常の動画書き出しにはデモサーバーは不要です。

## 素材

- ロゴ：`docs/public/brand/kikimimi-mark.svg` のコピー。
- フォント：Noto Sans JP（Google Fonts）。同梱の `public/fonts/OFL.txt` に従って使用・再配布します。フォントを同梱し、動画レンダリング時の外部通信を不要にしています。
- 説明内容：リポジトリのREADMEと画面実装に基づきます。計測可能な項目にはエージェント・データソースによる差があります。
- Xへの投稿文案：`post-ja.txt`。投稿は自動実行しません。

Remotionの基本構成と書き出しについては [fundamentals](https://www.remotion.dev/docs/the-fundamentals) と [render CLI](https://www.remotion.dev/docs/cli/render) を参照。
