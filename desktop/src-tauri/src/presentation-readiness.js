(generation, channel = 'cloud') => {
  // The native view stays hidden while the SPA loads its session, data and fonts.
  // This fixed navigation signal can only reveal this view; it grants no native API.
  let done = false;
  let lastLayout = '';
  let stableSamples = 0;
  let timer;
  const signal = (kind) => {
    if (done) return;
    done = true;
    clearTimeout(timer);
    clearTimeout(deadline);
    observer.disconnect();
    location.assign(`/__desktop_${channel}_${kind}?generation=${generation}`);
  };
  const deadline = setTimeout(() => signal('failed'), 30000);
  const observer = new MutationObserver(() => {
    stableSamples = 0;
  });
  const check = () => {
    if (done) return;
    const root = document.getElementById('root') || document.body;
    const loading = root?.querySelector('.boot-loading, .state-panel--loading, [aria-busy="true"]');
    if (!root?.childElementCount || loading || document.fonts?.status === 'loading') {
      stableSamples = 0;
    } else {
      const layout = [root, ...root.querySelectorAll('main, [aria-label="Workspace sidebar"], h1')]
        .map(el => {
          const r = el.getBoundingClientRect();
          return [r.x, r.y, r.width, r.height].map(Math.round).join(',');
        }).join(';');
      stableSamples = layout === lastLayout ? stableSamples + 1 : 0;
      lastLayout = layout;
      if (stableSamples >= 3) { signal('ready'); return; }
    }
    // Hidden WKWebViews may suspend animation frames; inspect layout on timers.
    timer = setTimeout(check, 50);
  };
  observer.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
  check();
}
