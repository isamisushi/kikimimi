import {useEffect, useState} from 'react';
import {AbsoluteFill, Easing, OffthreadVideo, Sequence, cancelRender, continueRender, delayRender, interpolate, staticFile, useCurrentFrame} from 'remotion';
import {scenes} from './demoTimeline';
import './demo.css';

export const DEMO_FPS = 30;
export const DEMO_DURATION = 76 * DEMO_FPS;
type Key = readonly [time:number,zoom:number,x:number,y:number];
// Coordinates are in the original 1440×720 recording. Begin and end at the
// full window; focus on the actual recorded UI while subtitles stay fixed.
const cameras: Record<typeof scenes[number]['name'],readonly Key[]> = {
  workspace:[[0,1,720,360],[.5,1,720,360],[1.2,1.65,450,230],[3,1.65,450,230],[3.8,1,720,360],[5.1,1,720,360],[6,1.45,650,280],[8.8,1.45,650,280],[9.8,1,720,360]],
  models:[[0,1,720,360],[2,1,720,360],[2.7,1.25,710,380],[4,1.25,710,380],[4.8,1,720,360],[7.2,1,720,360],[8.1,1.75,545,520],[12.8,1.75,545,520],[13.8,1,720,360]],
  extensions:[[0,1,720,360],[1.6,1,720,360],[2.4,1.6,585,295],[4.8,1.6,585,295],[5.7,1,720,360],[6.7,1,720,360],[7.4,1.6,550,275],[9.1,1.6,550,275],[10,1.65,575,515],[12.8,1.65,575,515],[13.8,1,720,360]],
  failures:[[0,1,720,360],[2,1,720,360],[2.8,1.4,725,295],[4,1.4,725,295],[4.8,1.8,1000,295],[7.8,1.8,1000,295],[8.8,1,720,360]],
  cli:[[0,1,720,360],[.5,1,720,360],[1.4,1.65,445,285],[6.7,1.65,445,285],[7.6,1,720,360],[8.1,1,720,360],[8.9,1.6,455,290],[10.8,1.6,455,290],[11.6,1.8,455,305],[15.8,1.8,455,305],[16.8,1,720,360]],
  storage:[[0,1,720,360],[3,1,720,360],[3.8,1.3,600,310],[5,1.3,600,310],[5.8,1,720,360],[8.5,1,720,360],[9.3,1.5,600,425],[10.8,1.5,600,425],[11.8,1,720,360]],
};

function DemoScene({scene}:{scene:typeof scenes[number]}) {
  const frame=useCurrentFrame();
  const keys=cameras[scene.name];
  const options={extrapolateLeft:'clamp' as const,extrapolateRight:'clamp' as const,easing:Easing.inOut(Easing.cubic)};
  const times=keys.map(k=>k[0]*DEMO_FPS);
  const zoom=interpolate(frame,times,keys.map(k=>k[1]),options);
  const x=interpolate(frame,times,keys.map(k=>k[2]),options);
  const y=interpolate(frame,times,keys.map(k=>k[3]),options);
  const cx=Math.max(720/zoom,Math.min(1440-720/zoom,x));
  const cy=Math.max(360/zoom,Math.min(720-360/zoom,y));
  const caption=[...scene.captions].reverse().find(([at])=>frame>=at*DEMO_FPS)?.[1];
  return <AbsoluteFill style={{background:scene.name==='cli'?'#101723':'#f5f6f8'}}>
    <div className="landscape-footage"><OffthreadVideo src={staticFile(`demo/${scene.name}.mp4`)} muted style={{position:'absolute',width:1920,height:960,maxWidth:'none',transformOrigin:'0 0',transform:`translate(${960-cx*4/3*zoom}px,${480-cy*4/3*zoom}px) scale(${zoom})`}}/></div>
    <div className="landscape-subtitle"><span>{caption}</span></div>
    <div className="landscape-fixture">架空のテストデータ</div>
  </AbsoluteFill>;
}

export function DemoVideo() {
  const [handle]=useState(()=>delayRender('Load Japanese subtitles'));
  useEffect(()=>{
    const font=new FontFace('Demo Noto',`url(${staticFile('fonts/NotoSansJP.ttf')})`,{weight:'100 900'});
    font.load().then(loaded=>{document.fonts.add(loaded);continueRender(handle);}).catch(cancelRender);
  },[handle]);
  return <AbsoluteFill className="landscape-demo">
    {scenes.map(scene=><Sequence key={scene.name} from={scene.from*DEMO_FPS} durationInFrames={scene.duration*DEMO_FPS}><DemoScene scene={scene}/></Sequence>)}
  </AbsoluteFill>;
}
