// Runs in the unprivileged dashboard views. Native navigation handles only this
// fixed route; this script has no access to native commands or credentials.
(() => {
  const install = () => {
    const nav = document.querySelector('.topbar__nav');
    if (!nav) return;
    let link = nav.querySelector('[data-desktop-workspace]');
    if (!link) {
      link = document.createElement('a');
      link.dataset.desktopWorkspace = 'true';
      link.className = 'topbar__link';
      link.href = '/__desktop_workspace';
      link.textContent = 'Workspace Settings';
      nav.appendChild(link);
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
