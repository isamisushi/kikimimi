// Runs in the unprivileged dashboard views. Native navigation handles only this
// fixed route; this script has no access to native commands or credentials.
(() => {
  window.__kikimimiDesktop = true;
  const install = () => {
    const nav = document.querySelector('.topbar__nav');
    if (!nav) return;
    // Mac preferences have one home in the shell's Settings tab.
    for (const [path, label] of [['/overview', 'ACTIVITY'], ['/team', 'CLOUD MANAGEMENT']]) {
      const anchor = nav.querySelector(`a[href="${path}"]`);
      if (anchor && !nav.querySelector(`[data-section="${path}"]`)) {
        const heading = document.createElement('span');
        heading.className = 'desktop-nav-label';
        heading.dataset.section = path;
        heading.textContent = label;
        nav.insertBefore(heading, anchor);
      }
    }
    for (const old of nav.querySelectorAll('a[href="/storage"]')) old.hidden = true;
  };
  document.addEventListener('click', event => {
    const link = event.target instanceof Element ? event.target.closest('[data-desktop-workspace]') : null;
    if (!link) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    location.assign('/__desktop_workspace');
  }, true);
  document.addEventListener('DOMContentLoaded', () => {
    const style = document.createElement('style');
    style.textContent = window.__desktopWorkspaceStyles;
    document.head.appendChild(style);
    install();
    new MutationObserver(install).observe(document.body, { childList: true, subtree: true });
  });
})();
