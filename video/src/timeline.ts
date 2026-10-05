export const FPS = 30;
export const scenes = [
  {id: 'intro', label: 'INTRO', seconds: 4},
  {id: 'workspace', label: 'PERSONAL / TEAM', seconds: 6},
  {id: 'models', label: 'MODELS', seconds: 6},
  {id: 'skills', label: 'SKILLS', seconds: 5},
  {id: 'mcp', label: 'MCP', seconds: 5},
  {id: 'subagents', label: 'SUBAGENTS', seconds: 5},
  {id: 'tools', label: 'TOOLS', seconds: 4},
  {id: 'privacy', label: 'LOCAL', seconds: 5},
  {id: 'outro', label: 'START', seconds: 5},
] as const;
export const timeline = scenes.map((scene, i) => ({
  ...scene,
  from: scenes.slice(0, i).reduce((sum, s) => sum + s.seconds * FPS, 0),
  duration: scene.seconds * FPS,
}));
export const DURATION = scenes.reduce((sum, s) => sum + s.seconds * FPS, 0);
