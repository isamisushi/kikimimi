import {useEffect, useState} from 'react';
import {AbsoluteFill, OffthreadVideo, Sequence, cancelRender, continueRender, delayRender, interpolate, Easing, staticFile, useCurrentFrame} from 'remotion';
import './vertical.css';
import {scenes} from './demoTimeline';

export const VERTICAL_DURATION = 76 * 30;


// Camera coordinates refer to the 900×1300 recording. Hold each focus long
// enough to read, then return to the full window before the next navigation.
type CameraKey = readonly [seconds:number, zoom:number, centerX:number, centerY:number];
const cameras: Record<typeof scenes[number]['name'], readonly CameraKey[]> = {
  workspace: [[0,1,450,650],[.5,1,450,650],[1.1,2.3,220,290],[3,2.3,220,290],[3.7,1,450,650],[5,1,450,650],[5.8,2.4,680,280],[8.8,2.4,680,280],[9.8,1,450,650]],
  models: [[0,1,450,650],[1.4,1,450,650],[2.2,1.5,450,510],[4,1.5,450,510],[4.8,1,450,650],[5.4,1,450,650],[6.2,2.4,225,845],[8,2.4,225,845],[9,2.4,610,845],[12.8,2.4,610,845],[13.8,1,450,650]],
  extensions: [[0,1,450,650],[1.6,1,450,650],[2.4,2.4,235,290],[4.8,2.4,235,290],[5.7,1,450,650],[6.7,1,450,650],[7.5,2.4,235,280],[9.1,2.4,235,280],[10,2.4,215,530],[12.8,2.4,215,530],[13.8,1,450,650]],
  failures: [[0,1,450,650],[1.5,1,450,650],[2.3,2.3,230,290],[3.3,2.3,230,290],[4.2,2.6,680,270],[7.8,2.6,680,270],[8.8,1,450,650]],
  cli: [[0,1,450,650],[.5,1,450,650],[1.4,1.65,285,400],[3.3,1.65,285,400],[4.1,2.2,230,300],[6.8,2.2,230,300],[7.6,1,450,650],[8.1,1,450,650],[8.9,1.8,280,365],[10.8,1.8,280,365],[11.6,2.6,440,285],[15.8,2.6,440,285],[16.8,1,450,650]],
  storage: [[0,1,450,650],[2.5,1,450,650],[3.3,2,240,325],[5,2,240,325],[5.8,1,450,650],[7,1,450,650],[8.7,2.3,230,665],[10.8,2.3,230,665],[11.8,1,450,650]],
};

function cameraAt(name:typeof scenes[number]['name'], frame:number) {
  const keys=cameras[name];
  const times=keys.map(k=>k[0]*30);
  const options={extrapolateLeft:'clamp' as const,extrapolateRight:'clamp' as const,easing:Easing.inOut(Easing.cubic)};
  const zoom=interpolate(frame,times,keys.map(k=>k[1]),options);
  const x=interpolate(frame,times,keys.map(k=>k[2]),options);
  const y=interpolate(frame,times,keys.map(k=>k[3]),options);
  // Constrain the crop to recorded pixels; never expose empty canvas edges.
  const cx=Math.max(450/zoom,Math.min(900-450/zoom,x));
  const cy=Math.max(650/zoom,Math.min(1300-650/zoom,y));
  return {zoom, left:540-cx*1.2*zoom, top:780-cy*1.2*zoom};
}

function DemoScene({scene}: {scene:typeof scenes[number]}) {
  const frame=useCurrentFrame();
  const caption=[...scene.captions].reverse().find(([at])=>frame>=at*30)?.[1];
  const camera=cameraAt(scene.name,frame);
  return <AbsoluteFill>
    <div className="vertical-recording"><OffthreadVideo src={staticFile(`demo-portrait/${scene.name}.mp4`)} muted style={{position:'absolute',width:1080,height:1560,maxWidth:'none',transformOrigin:'0 0',transform:`translate(${camera.left}px,${camera.top}px) scale(${camera.zoom})`}}/></div>
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
