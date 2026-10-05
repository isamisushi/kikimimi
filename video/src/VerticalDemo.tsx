import {useEffect, useState} from 'react';
import {AbsoluteFill, OffthreadVideo, Sequence, cancelRender, continueRender, delayRender, staticFile, useCurrentFrame} from 'remotion';
import './vertical.css';

export const VERTICAL_DURATION = 76 * 30;
const scenes = [
  {name:'workspace', from:0, duration:10, captions:[
    [0, 'kikimimiで、個人のAI利用を確認。'],
    [3.5, 'チームに切り替えて、\nメンバーごとの利用状況を見ます。'],
    [6, 'Renの使用量が多め。\nどんな処理に使われているか調べます。'],
  ]},
  {name:'models', from:10, duration:14, captions:[
    [0, 'Modelsで、使われたモデルと\nトークンの推移を確認します。'],
    [5, 'In subagentsで並べ替えると、\nサブエージェントでの利用も分かります。'],
    [9, 'このOpusのリクエストは、\n100%サブエージェント由来でした。'],
  ]},
  {name:'extensions', from:24, duration:14, captions:[
    [0, 'MCPを見ると、notionは設定済みでも\n一度も呼ばれていません。'],
    [6, 'Skillsも確認。\ncode-reviewは21回使われています。'],
    [10, 'release-notesは未使用。\n設定を見直す候補が見つかります。'],
  ]},
  {name:'failures', from:38, duration:9, captions:[
    [0, 'Toolsを失敗数で並べ替えて、\n調べるべきツールを見つけます。'],
    [4, 'Playwrightは42回中6回失敗。\nまずはここから調べます。'],
  ]},
  {name:'cli', from:47, duration:17, captions:[
    [0, '同じデータをCLIでも確認。\nkikimimi query unused-mcp を実行します。'],
    [4, 'notionは呼び出し0回。\n画面と同じ結果です。'],
    [7.7, '続けて kikimimi query tools。'],
    [11.5, 'こちらでも42回中6回失敗。\nローカルのParquetを直接集計できます。'],
  ]},
  {name:'storage', from:64, duration:12, captions:[
    [0, 'macOSアプリからも分析できます。\nデスクトップ版は開発プレビューです。'],
    [3, '履歴はローカルに保持。\nクラウドで個人・チームのデータを共有。'],
    [6, 'さらに、自分のS3にも保存できます。\nクラウド・S3への送信は任意です。'],
    [9.5, '使い方に合う保存先を選べます。\nkikimimi.dev'],
  ]},
] as const;

function DemoScene({scene}: {scene:typeof scenes[number]}) {
  const frame=useCurrentFrame();
  const caption=[...scene.captions].reverse().find(([at])=>frame>=at*30)?.[1];
  return <AbsoluteFill>
    <div className="vertical-recording"><OffthreadVideo src={staticFile(`demo-portrait/${scene.name}.mp4`)} muted style={{width:'100%',height:'100%'}}/></div>
    <div className="vertical-subtitles">{caption}</div>
  </AbsoluteFill>;
}

export function VerticalDemo() {
  const [handle]=useState(()=>delayRender('Load Japanese subtitles'));
  useEffect(()=>{
    const font=new FontFace('Vertical Noto',`url(${staticFile('fonts/NotoSansJP.ttf')})`,{weight:'100 900'});
    font.load().then(loaded=>{document.fonts.add(loaded);continueRender(handle);}).catch(cancelRender);
  },[handle]);
  return <AbsoluteFill className="vertical-demo">
    {scenes.map(scene=><Sequence key={scene.name} from={scene.from*30} durationInFrames={scene.duration*30}><DemoScene scene={scene}/></Sequence>)}
    <div className="vertical-note">架空のテストデータで操作しています</div>
  </AbsoluteFill>;
}
