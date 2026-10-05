# kikimimi 紹介動画 — X / 日本語

Remotionで編集・再生成できる30秒の紹介動画です。1920×1080、30fps、H.264 / yuv420p のMP4。音声なしで内容が伝わるテロップ構成で、BGM・ナレーションは含めていません。

## プレビューと書き出し

```sh
npm --prefix video ci
npm --prefix video run studio
npm --prefix video run check
npm --prefix video run render
npm --prefix video run poster
```

出力は `video/out/kikimimi-ja.mp4` と `video/out/poster.png`。出力ファイルと依存パッケージはGit管理対象外です。

Chromeを自動ダウンロードできない環境では、インストール済みのChromeを指定できます。

```sh
npm --prefix video run render -- --browser-executable=/usr/bin/google-chrome --concurrency=2
```

## 構成

| 時間 | 内容 |
| --- | --- |
| 0–4秒 | AIエージェント、何に時間を使ってる？ |
| 4–10秒 | Overview：活動・失敗・トークン使用量 |
| 10–15秒 | Tools：呼び出し回数・失敗数・所要時間 |
| 15–20秒 | MCP：設定済みで未使用のサーバー |
| 20–25秒 | ローカル保存、収集しない内容、任意のクラウド送信 |
| 25–30秒 | ロゴ、開始コマンド、kikimimi.dev |

文言とアニメーションは `src/Video.tsx`、スタイルは `src/style.css`、尺・サイズ・fpsは `src/Root.tsx` で変更します。アニメーションはフレーム数から算出し、再生時刻や乱数に依存しません。

## アプリ画面の再撮影

`public/screenshots/` はこのリポジトリの実際のReact UIを、既存のモックAPIの合成データで撮影したものです。本番の利用データやアカウントは使用していません。撮影時のみ `/web/me` のローカル表示を有効にし、任意機能のサブスクリプション使用量パネルを無効にして、グラフが見える構成にしています。UI本体は変更していません。

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
