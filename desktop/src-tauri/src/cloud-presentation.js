(() => {
  // Presentation only. Cloud keeps its own session, workspace selector and
  // navigation; the outer app owns branding. Never apply this to OAuth pages.
  if (location.origin !== 'https://kikimimi.dev') return;
  window.__KIKIMIMI_DESKTOP__ = true;
  const install = () => {
    if (document.getElementById('kikimimi-desktop-presentation')) return;
    const style = document.createElement('style');
    style.id = 'kikimimi-desktop-presentation';
    style.textContent = `
      [data-slot="sidebar-header"] a:has(.cloud-brand-mark),
      [data-slot="sidebar-header"] > div:has(.cloud-brand-mark):not(:has(button)),
      .login-card__brand { display: none !important; }

      /* The page already has a heading. Keep the collapse/open control beside
         it without adding a second title bar, including in the mobile layout. */
      [data-slot="sidebar-inset"] > header:has(> [data-sidebar="trigger"]) {
        position: absolute; top: 32px; right: 24px; z-index: 1;
        height: 28px; padding: 0; border: 0; background: transparent;
      }
      [data-slot="sidebar-inset"] > header:has(> [data-sidebar="trigger"]) > span {
        display: none;
      }
      [data-slot="sidebar-inset"] #main-content .page__header {
        padding-right: 40px;
      }
    `;
    document.head.appendChild(style);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', install, { once: true });
  else install();
})();
