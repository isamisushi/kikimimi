import {useEffect, useState, type CSSProperties, type ReactNode} from 'react';
import {AbsoluteFill, Img, Sequence, cancelRender, continueRender, delayRender, interpolate, spring, staticFile, useCurrentFrame} from 'remotion';
import './style.css';
import {DURATION, FPS, timeline} from './timeline';

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
  const panel = page.endsWith('-panel') || page.startsWith('model-');
  const scale = interpolate(f, [0, 180], [0.98, 1.015], clamp);
  return <div className="window" style={{width, transform: `scale(${scale})`}}>
    <div className="window-bar"><i/><i/><i/><span>kikimimi / {page}</span><b>DEMO</b></div>
    <div style={{height, overflow: 'hidden', position: 'relative', background: panel ? '#fff' : '#f5f6f8'}}>
      <Img src={staticFile(`screenshots/${page}.png`)} style={{width: '100%', position: 'absolute', top, ...(panel ? {height: '100%', objectFit: 'contain', objectPosition: 'center'} as CSSProperties : {})}} />
    </div>
  </div>;
}

function Intro() {
  const f = useCurrentFrame();
  return <Scene duration={120}>
    <div className="intro-orbit" style={{transform: `rotate(${f * 0.12}deg)`}}><div/><div/><div/></div>
    <div className="intro-copy">
      <Reveal><div className="eyebrow">AI CODING AGENT OBSERVABILITY</div></Reveal>
      <Reveal delay={4}><h1>AIエージェント、<br/>何を、どれだけ<br/><em>使ってる？</em></h1></Reveal>
      <Reveal delay={15}><p>Claude Code と Codex CLI の活動を可視化。</p></Reveal>
    </div>
    <Reveal delay={12} style={{position: 'absolute', left: 1265, top: 320}}>
      <div className="hero-mark"><Img src={staticFile('mark.svg')}/></div>
    </Reveal>
    <Reveal delay={26} style={{position: 'absolute', left: 1220, top: 640}}><div className="agent-chip">Claude Code <span>+</span> Codex CLI</div></Reveal>
  </Scene>;
}

function Workspaces() {
  const f = useCurrentFrame();
  const team = f >= 90;
  return <Scene duration={180}>
    <div className="wide-heading"><Reveal><div className="eyebrow">01 / PERSONAL & TEAM</div><h2>自分の活用も、チームの活用も。</h2></Reveal>
      <Reveal delay={8}><p>個人・チームのワークスペースを切り替えて、利用状況を把握。</p></Reveal>
    </div>
    <div className="workspace-tabs"><span className={!team ? 'selected' : ''}>個人 / Personal</span><span className={team ? 'selected' : ''}>チーム / Acme Inc</span></div>
    <div style={{position: 'absolute', left: 205, top: 435}}><Window page={team ? 'team' : 'personal'} height={440}/></div>
    <div className="bottom-note">クラウドの実画面 / デモデータ。チームのメンバー別利用状況は管理者・オーナー向け。</div>
  </Scene>;
}

const analysisCopy = {
  models: {label: '02 / MODELS', title: 'どのモデルが、使われている？', body: 'モデル別の使用量・コスト・推論の強度（effort）を分析。', image: 'model-chart', duration: 180, tags: ['モデル別トークン', '日ごとの推移', 'コスト・effort'], note: 'デモデータ / モデル分析はClaude CodeのAPIリクエスト記録に基づきます。'},
  skills: {label: '03 / SKILLS', title: 'どのスキルが、活用されている？', body: '呼び出し回数・失敗数・利用セッションを確認。設定だけのスキルも見直せます。', image: 'skills-panel', duration: 150, tags: ['呼び出し回数', '利用セッション', '未使用スキル'], note: '実際のアプリ画面 / デモデータ。取得できる項目はエージェントとデータソースによって異なります。'},
  mcp: {label: '04 / MCP SERVERS', title: 'MCPの使われ方も、見えてくる。', body: 'サーバーごとの利用状況を分析。設定済みで未使用のMCPも発見。', image: 'mcp-panel', duration: 150, tags: ['サーバー別の利用回数', '失敗数', '未使用MCP'], note: '実際のアプリ画面 / デモデータ'},
  subagents: {label: '05 / SUBAGENT MODELS', title: 'サブエージェントのモデルまで。', body: 'モデル別に、サブエージェント内の使用割合とトークンを確認。', image: 'model-detail', duration: 150, tags: ['使用モデル', 'In subagents', 'トークン使用量'], note: 'デモデータ / Claude Codeで記録された項目が対象。未取得の値は不明として表示します。'},
  tools: {label: '06 / TOOLS', title: '改善の手がかりを、データから。', body: 'ツールの呼び出し回数・失敗数・所要時間を比較。', image: 'tools-panel', duration: 120, tags: ['呼び出し回数', '失敗数', 'p50 / p95'], note: '実際のアプリ画面 / デモデータ'},
} as const;

function Analysis({kind}: {kind: keyof typeof analysisCopy}) {
  const c = analysisCopy[kind];
  return <Scene duration={c.duration}>
    <div className="wide-heading"><Reveal><div className="eyebrow">{c.label}</div><h2>{c.title}</h2></Reveal>
      <Reveal delay={8}><p>{c.body}</p></Reveal></div>
    <Reveal delay={10} style={{position: 'absolute', left: 205, top: 398}}>
      <Window page={c.image} height={kind === 'models' ? 430 : 380}/>
    </Reveal>
    <Reveal delay={22} style={{position: 'absolute', top: 886, width: '100%'}}><div className="analysis-tags">{c.tags.map(tag => <span key={tag}>{tag}</span>)}</div></Reveal>
    <div className="bottom-note" style={{bottom: 88}}>{c.note}</div>
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
    {timeline.map(s => <Sequence key={s.id} from={s.from} durationInFrames={s.duration}>
      {s.id === 'intro' ? <Intro/> : s.id === 'workspace' ? <Workspaces/> : s.id === 'privacy' ? <Privacy/> : s.id === 'outro' ? <Outro/> : <Analysis kind={s.id}/>}
    </Sequence>)}
    <footer><span>kikimimi</span><div className="chapters">{timeline.map(s => {
      const active = frame >= s.from && frame < s.from + s.duration;
      return <span key={s.id} style={{color: active ? BLUE : '#9da1aa'}}>{s.label}</span>;
    })}</div><span>日本語 / {DURATION / FPS} SEC</span></footer>
    <div className="progress" style={{width: `${frame / (DURATION - 1) * 100}%`}}/>
  </AbsoluteFill>;
}
