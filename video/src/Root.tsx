import {Composition} from 'remotion';
import {KikimimiVideo} from './Video';
import {DURATION, FPS} from './timeline';

export const Root = () => <Composition id="KikimimiJa" component={KikimimiVideo}
  width={1920} height={1080} fps={FPS} durationInFrames={DURATION} />;
