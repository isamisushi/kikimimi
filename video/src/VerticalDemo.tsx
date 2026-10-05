import {useEffect, useState} from 'react';
import {AbsoluteFill, Img, OffthreadVideo, Sequence, cancelRender, continueRender, delayRender, interpolate, staticFile, useCurrentFrame} from 'remotion';
import {clips} from './DemoVideo';
import './vertical.css';
export const VERTICAL_DURATION = 81 * 30;
const details = {
  workspace: {title:'個人から、チームへ。',result:'メンバーごとの利用を比較',x:145,y:90},
  models: {title:'どのモデルを、\nどこで使った？',result:'Opusは100%サブエージェント由来',x:165,y:405},
  extensions: {title:'MCPも、Skillsも。\n使われ方が見える。',result:'設定済みでも、未使用のものを発見',x:150,y:380},
  failures: {title:'失敗の多いツールを\n見つける。',result:'Playwright：42回中6回失敗',x:735,y:155},
  cli: {title:'同じデータを、\nCLIでも。',result:'画面とCLIで、同じ結果を確認',x:40,y:140},
};
function Clip({clip}:{clip:typeof clips[number]}) {
  const f=useCurrentFrame(); const t=f/30; const d=details[clip.name];
  const caption=[...clip.captions].reverse().find(([time])=>t>=time)?.[1];
  let x=d.x,y=d.y;
  if(clip.name==='workspace') {x=t<4?145:160;y=t<4?0:145;}
  if(clip.name==='models' && t<6) {x=150;y=85;}
  if(clip.name==='extensions') {x=t<6?150:150;y=t<6?160:t<10?95:380;}
  if(clip.name==='failures' && t<3) {x=150;y=100;}
  if(clip.name==='cli') {x=t<11.5?40:455;y=t<11.5?145:210;}
  const scale=clip.name==='models'?1.4:1.6;
  return <AbsoluteFill>
    <div className="v-chapter">{clip.chapter}</div>
    <h1 className="v-title">{d.title}</h1>
    <div className="v-overview"><OffthreadVideo src={staticFile(`demo/${clip.name}.mp4`)} muted style={{width:'100%',height:'100%'}}/></div>
    <div className="v-detail-label">{clip.name==='cli'?'コマンドと実行結果':'注目エリアを拡大'}</div>
    <div className="v-focus" style={{background:clip.name==='cli'?'#111827':'#fff'}}><OffthreadVideo src={staticFile(`demo/${clip.name}.mp4`)} muted style={{position:'absolute',width:1440*scale,height:720*scale,maxWidth:'none',left:-x*scale,top:-y*scale}}/></div>
    <div className="v-result">{d.result}</div>
    <div className="v-caption">{caption}</div>
  </AbsoluteFill>;
}
function Storage(){
  const f=useCurrentFrame();
  return <AbsoluteFill className="v-storage">
    <div className="v-chapter">04 / 使い方も、保存先も選べる</div>
    <h1 className="v-title">手元でも。<br/>チームでも。</h1>
    <div className="v-desktop"><span>DESKTOP + WEB + CLI</span><strong>macOSアプリでも分析</strong><small>デスクトップ版は開発プレビュー</small></div>
    <div className="v-storage-card"><span>いつも手元に</span><strong>ローカル</strong><p>Parquetで履歴を保持<br/>ローカル利用はアカウント不要</p></div>
    <div className="v-arrow">↓ <span>必要に応じて追加保存</span></div>
    <div className="v-destinations" style={{opacity:interpolate(f,[25,45],[0,1],{extrapolateLeft:'clamp',extrapolateRight:'clamp'})}}>
      <div><span>個人・チームで共有</span><strong>クラウド</strong></div>
      <div><span>自分のインフラに</span><strong>自分のS3</strong></div>
    </div>
    <div className="v-caption">ローカルだけでも利用可能。<br/>クラウド・S3への送信は任意です。</div>
  </AbsoluteFill>;
}
function Bookend({end=false}:{end?:boolean}){return <AbsoluteFill className="v-bookend"><div className="v-eyebrow">{end?'AI開発の利用状況を、ひとつに。':'実画面とCLIで見る、3つの使い方'}</div><h1>{end?<>見つける。<br/>確かめる。<br/><em>AI開発を、<br/>見直す。</em></>:<>AIの使い方、<br/><em>見えてますか？</em></>}</h1><p>{end?'kikimimi.dev ↗':'個人・チーム / モデル / MCP / Skills'}</p><small>{end?'Web・CLI・macOS開発プレビュー':'3人・7日分・21セッションの架空データ'}</small></AbsoluteFill>}
export function VerticalDemo(){
  const f=useCurrentFrame();const [handle]=useState(()=>delayRender('Load vertical Japanese captions'));
  useEffect(()=>{const font=new FontFace('Vertical Noto',`url(${staticFile('fonts/NotoSansJP.ttf')})`,{weight:'100 900'});font.load().then(loaded=>{document.fonts.add(loaded);continueRender(handle);}).catch(cancelRender);},[handle]);
  return <AbsoluteFill className="v-canvas">
    <Sequence durationInFrames={90}><Bookend/></Sequence>
    {clips.map(clip=><Sequence key={clip.name} from={clip.from*30} durationInFrames={clip.duration*30}><Clip clip={clip}/></Sequence>)}
    <Sequence from={67*30} durationInFrames={300}><Storage/></Sequence>
    <Sequence from={77*30} durationInFrames={120}><Bookend end/></Sequence>
    <div className="v-brand"><Img src={staticFile('mark.svg')}/>kikimimi<span>操作デモ</span></div>
    <div className="v-footer">{f<67*30?'テストデータ / 実操作収録':'ローカル・クラウド・自分のS3'}</div>
    <div className="v-progress" style={{width:`${100*f/(VERTICAL_DURATION-1)}%`}}/>
  </AbsoluteFill>;
}
