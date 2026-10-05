import {useEffect, useState, type CSSProperties, type ReactNode} from 'react';
import {AbsoluteFill, Img, Sequence, cancelRender, continueRender, delayRender, interpolate, spring, staticFile, useCurrentFrame} from 'remotion';
import './style.css';

const BLUE = '#3457d5';
const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;
const ease = (f: number, start = 0, end = 22) => interpolate(f, [start, end], [0, 1], clamp);

function Reveal({children, delay = 0, style}: {children: ReactNode; delay?: number; style?: CSSProperties}) {
  const f = useCurrentFrame();
  const s = spring({frame: f - delay, fps: 30, config: {damping: 22, stiffness: 110}});
  return <div style={{opacity: ease(f, delay, delay + 14), transform: `translateY(${(1 - s) * 35}px)`, ...style}}>{children}</div>;
}

function Brand({large = false}: {large?: boolean}) {
  return <div className={`brand ${large ? 'brand-large' : ''}`}><Img src={staticFile('mark.svg')} /><span>kikimimi</span></div>;
}

function Scene({children, duration}: {children: ReactNode; duration: number}) {
  const f = useCurrentFrame();
  return <AbsoluteFill style={{opacity: interpolate(f, [0, 10, duration - 10, duration - 1], [0, 1, 1, 0], clamp)}}>{children}</AbsoluteFill>;
}

function Window({page, top = 0, width = 1510, height = 635}: {page: string; top?: number; width?: number; height?: number}) {
  const f = useCurrentFrame();
  const scale = interpolate(f, [0, 180], [0.98, 1.015], clamp);
  return <div className="window" style={{width, transform: `scale(${scale})`}}>
    <div className="window-bar"><i/><i/><i/><span>kikimimi / {page}</span><b>DEMO</b></div>
    <div style={{height, overflow: 'hidden', position: 'relative', background: '#f5f6f8'}}>
      <Img src={staticFile(`screenshots/${page}.png`)} style={{width: '100%', position: 'absolute', top}} />
    </div>
  </div>;
}

function Intro() {
  const f = useCurrentFrame();
  return <Scene duration={120}>
    <div className="intro-orbit" style={{transform: `rotate(${f * 0.12}deg)`}}><div/><div/><div/></div>
    <div className="intro-copy">
      <Reveal><div className="eyebrow">AI CODING AGENT OBSERVABILITY</div></Reveal>
      <Reveal delay={4}><h1>AIエージェント、<br/>何に時間を<br/><em>使ってる？</em></h1></Reveal>
      <Reveal delay={15}><p>Claude Code と Codex CLI の活動を可視化。</p></Reveal>
    </div>
    <Reveal delay={12} style={{position: 'absolute', left: 1265, top: 320}}>
      <div className="hero-mark"><Img src={staticFile('mark.svg')}/></div>
    </Reveal>
    <Reveal delay={26} style={{position: 'absolute', left: 1220, top: 640}}><div className="agent-chip">Claude Code <span>+</span> Codex CLI</div></Reveal>
  </Scene>;
}

function Overview() {
  return <Scene duration={180}>
    <div className="wide-heading"><Reveal><div className="eyebrow">01 / OVERVIEW</div><h2>エージェントの活動を、ひと目で。</h2></Reveal>
      <Reveal delay={8}><p>ツール呼び出し・失敗・トークン使用量をダッシュボードに。</p></Reveal>
    </div>
    <Reveal delay={8} style={{position: 'absolute', left: 205, top: 390}}><Window page="overview" top={-160} height={505}/></Reveal>
    <div className="bottom-note">画面はデモデータです。取得できる項目はエージェントとデータソースによって異なります。</div>
  </Scene>;
}

function Detail({kind}: {kind: 'tools' | 'mcp'}) {
  const f = useCurrentFrame();
  const tools = kind === 'tools';
  return <Scene duration={150}>
    <div className="detail-copy">
      <Reveal><div className="eyebrow">{tools ? '02 / TOOLS' : '03 / MCP SERVERS'}</div></Reveal>
      <Reveal delay={4}><h2>{tools ? <>失敗の多い<br/>ツールを、<br/><em>見つける。</em></> : <>そのMCP、<br/>本当に<br/><em>使ってる？</em></>}</h2></Reveal>
      <Reveal delay={12}><p>{tools ? <>呼び出し回数・失敗数・所要時間。<br/>改善の手がかりを、データから。</> : <>設定したままの未使用サーバーを発見。<br/>構成を見直すきっかけに。</>}</p></Reveal>
      <Reveal delay={24}><div className={`signal ${tools ? 'red' : 'amber'}`}><span/>{tools ? 'Failures / p50 / p95' : 'Configured, but unused'}</div></Reveal>
    </div>
    <Reveal delay={10} style={{position: 'absolute', left: 720, top: 256}}><Window page={kind} width={1100} height={570} top={-45}/></Reveal>
    <div className="focus-rule" style={{left: tools ? 1456 : 860, top: tools ? 474 : 470, width: tools ? 106 : 405, opacity: ease(f, 40, 55)}}/>
    <div className="bottom-note">実際のアプリ画面 / デモデータ</div>
  </Scene>;
}

function Privacy() {
  const f = useCurrentFrame();
  return <Scene duration={150}>
    <div className="wide-heading"><Reveal><div className="eyebrow">LOCAL FIRST</div><h2>活動のメタデータを、手元に。</h2></Reveal><Reveal delay={7}><p>プロンプト・コード・ツール出力は収集イベントに保存しません。</p></Reveal></div>
    <div className="privacy-flow">
      <Reveal delay={10}><div className="flow-card"><div className="flow-icon">&gt;_</div><h3>Claude Code<br/>Codex CLI</h3><p>いつもどおりに開発</p></div></Reveal>
      <div className="flow-line"><div style={{width: `${ease(f, 22, 50) * 100}%`}}/><span>活動メタデータ</span></div>
      <Reveal delay={22}><div className="flow-card highlighted"><Img src={staticFile('mark.svg')}/><h3>kikimimi</h3><p>ローカルに保存・可視化</p></div></Reveal>
    </div>
    <Reveal delay={45} style={{position: 'absolute', top: 846, width: '100%', textAlign: 'center'}}><div className="privacy-pills"><span>ローカル利用はアカウント不要</span><span>クラウドへの送信は任意</span></div></Reveal>
  </Scene>;
}

function Outro() {
  return <Scene duration={150}>
    <div className="outro">
      <Reveal><Brand large/></Reveal>
      <Reveal delay={7}><h2>AI開発に、<em>観察する力を。</em></h2></Reveal>
      <Reveal delay={14}><div className="terminal"><span>まずはローカルで</span><code><b>$</b> kikimimi init</code><code><b>$</b> kikimimi web</code></div></Reveal>
      <Reveal delay={23}><div className="cta">kikimimi.dev <span>↗</span></div><p className="install-note">インストール手順は公式サイトへ</p></Reveal>
    </div>
  </Scene>;
}

export function KikimimiVideo() {
  const frame = useCurrentFrame();
  const [fontHandle] = useState(() => delayRender('Load bundled Japanese font'));
  useEffect(() => {
    const font = new FontFace('Noto Sans JP', `url(${staticFile('fonts/NotoSansJP.ttf')})`, {weight: '100 900'});
    font.load().then((loaded) => {document.fonts.add(loaded); continueRender(fontHandle);}).catch(cancelRender);
  }, [fontHandle]);
  return <AbsoluteFill className="canvas">
    <div className="grid"/>
    <header><Brand/><span>AIの働きに、耳をすます。</span></header>
    <Sequence from={0} durationInFrames={120}><Intro/></Sequence>
    <Sequence from={120} durationInFrames={180}><Overview/></Sequence>
    <Sequence from={300} durationInFrames={150}><Detail kind="tools"/></Sequence>
    <Sequence from={450} durationInFrames={150}><Detail kind="mcp"/></Sequence>
    <Sequence from={600} durationInFrames={150}><Privacy/></Sequence>
    <Sequence from={750} durationInFrames={150}><Outro/></Sequence>
    <footer><span>kikimimi</span><div className="chapters">{['INTRO', 'OVERVIEW', 'TOOLS', 'MCP', 'LOCAL', 'START'].map((label, i) => {
      const starts = [0, 120, 300, 450, 600, 750, 900];
      const active = frame >= starts[i] && frame < starts[i + 1];
      return <span key={label} style={{color: active ? BLUE : '#9da1aa'}}>{label}</span>;
    })}</div><span>日本語 / 30 SEC</span></footer>
    <div className="progress" style={{width: `${frame / 899 * 100}%`}}/>
  </AbsoluteFill>;
}
