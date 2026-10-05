import {useEffect, useState} from 'react';
import {AbsoluteFill, Img, OffthreadVideo, Sequence, cancelRender, continueRender, delayRender, interpolate, staticFile, useCurrentFrame} from 'remotion';
import './demo.css';

export const DEMO_FPS = 30;
export const DEMO_DURATION = 71 * DEMO_FPS;
export const clips = [
  {name: 'workspace', from: 3, duration: 10, chapter: '01 / チームとモデルの使い方', captions: [
    [0, 'まずは個人のOverview。Studio Demoへ切り替えます。'],
    [3.5, 'チーム全体を表示。Membersでメンバーごとの利用状況へ。'],
    [6, 'Renの使用量が多め。どんな処理に使われているか調べます。'],
  ]},
  {name: 'models', from: 13, duration: 14, chapter: '01 / チームとモデルの使い方', captions: [
    [0, 'Modelsを開き、モデル別のトークン推移を確認。'],
    [5, '下へスクロールし、In subagentsで並べ替えます。'],
    [9, 'Opusのリクエストは100%サブエージェント由来。構成を見直す手がかりに。'],
  ]},
  {name: 'extensions', from: 27, duration: 14, chapter: '02 / 使われていない拡張機能を探す', captions: [
    [0, 'MCPを開くと、notionは設定済みなのに呼び出し0回。'],
    [6, 'Skillsでは、code-reviewが21回呼ばれています。'],
    [10, 'release-notesは設定済みで未使用。拡張機能の見直し候補です。'],
  ]},
  {name: 'failures', from: 41, duration: 9, chapter: '03 / 失敗しているツールを調べる', captions: [
    [0, 'ToolsのFailuresで並べ替え。失敗の多いツールを先頭に。'],
    [4, 'Playwrightは42回中6回失敗、p95は30秒。まずここから調べます。'],
  ]},
  {name: 'cli', from: 50, duration: 17, chapter: '03 / 同じデータをCLIでも確認', captions: [
    [0, 'ターミナルで kikimimi query unused-mcp。notionは0回。'],
    [7.7, '続けて kikimimi query tools を実行します。'],
    [11.5, '同じParquetを集計。画面と同じ42回・6失敗を確認できます。'],
  ]},
] as const;

function DemoClip({clip}: {clip: typeof clips[number]}) {
  const frame = useCurrentFrame();
  const caption = [...clip.captions].reverse().find(([time]) => frame >= time * DEMO_FPS)?.[1];
  // Camera moves magnify the recorded pixels; application state is never recreated.
  const focus = {
    workspace: {at:6, scale:1.2, origin:'25% 30%'},
    models: {at:9, scale:1.42, origin:'20% 80%'},
    extensions: {at:10.5, scale:1.2, origin:'25% 55%'},
    failures: {at:5, scale:1.3, origin:'20% 30%'},
    cli: {at:11.5, scale:1.06, origin:'10% 30%'},
  }[clip.name];
  const zoom = interpolate(frame, [focus.at * DEMO_FPS, (focus.at + .8) * DEMO_FPS], [1, focus.scale], {extrapolateLeft:'clamp', extrapolateRight:'clamp'});
  return <AbsoluteFill>
    <div className="demo-chapter">{clip.chapter}</div>
    <div className="demo-screen"><OffthreadVideo src={staticFile(`demo/${clip.name}.mp4`)} muted style={{width:'100%', height:'100%', transform:`scale(${zoom})`, transformOrigin:focus.origin}}/></div>
    <div className="demo-caption">{caption}</div>
  </AbsoluteFill>;
}

function Bookend({end = false}: {end?: boolean}) {
  const f = useCurrentFrame();
  const opacity = interpolate(f, [0, 9], [0, 1], {extrapolateRight:'clamp'});
  return <AbsoluteFill className="demo-bookend" style={{opacity}}>
    <div className="demo-kicker">{end ? 'kikimimi' : '実画面 + 実CLI / 3つの使い方'}</div>
    <h1>{end ? <>見つける。確かめる。<br/><em>AI開発を、見直す。</em></> : <>チームのAI利用、<br/><em>どこを見直す？</em></>}</h1>
    <p>{end ? 'kikimimi.dev ↗' : '個人・チーム → モデル・拡張機能 → CLI'}</p>
    <small>{end ? 'ローカル利用はアカウント不要。クラウド送信は任意。' : '3人 / 7日分 / 21セッションの架空データで操作します。'}</small>
  </AbsoluteFill>;
}

export function DemoVideo() {
  const frame = useCurrentFrame();
  const [handle] = useState(() => delayRender('Load Japanese demo captions'));
  useEffect(() => {
    const font = new FontFace('Demo Noto', `url(${staticFile('fonts/NotoSansJP.ttf')})`, {weight:'100 900'});
    font.load().then(loaded => {document.fonts.add(loaded); continueRender(handle);}).catch(cancelRender);
  }, [handle]);
  return <AbsoluteFill className="demo-canvas">
    <div className="demo-brand"><Img src={staticFile('mark.svg')}/>kikimimi<span>操作デモ</span></div>
    <div className="demo-fixture">テストデータ / 実操作収録</div>
    <Sequence durationInFrames={90}><Bookend/></Sequence>
    {clips.map(clip => <Sequence key={clip.name} from={clip.from * DEMO_FPS} durationInFrames={clip.duration * DEMO_FPS}><DemoClip clip={clip}/></Sequence>)}
    <Sequence from={67 * DEMO_FPS} durationInFrames={4 * DEMO_FPS}><Bookend end/></Sequence>
    <div className="demo-progress" style={{width:`${100 * frame / (DEMO_DURATION - 1)}%`}}/>
  </AbsoluteFill>;
}
