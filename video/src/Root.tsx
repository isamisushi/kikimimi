import {Composition} from 'remotion';
import {KikimimiVideo} from './Video';

export const Root = () => <Composition id="KikimimiJa" component={KikimimiVideo}
  width={1920} height={1080} fps={30} durationInFrames={900} />;
