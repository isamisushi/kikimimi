import {Composition} from 'remotion';
import {KikimimiVideo} from './Video';
import {DURATION, FPS} from './timeline';
import {DemoVideo, DEMO_DURATION, DEMO_FPS} from './DemoVideo';

export const Root = () => <>
  <Composition id="KikimimiJa" component={KikimimiVideo}
    width={1920} height={1080} fps={FPS} durationInFrames={DURATION} />
  <Composition id="KikimimiDemoJa" component={DemoVideo}
    width={1920} height={1080} fps={DEMO_FPS} durationInFrames={DEMO_DURATION} />
</>;
