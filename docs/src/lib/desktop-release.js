const repository = 'https://github.com/isamisushi/kikimimi';

// Ignore the CLI releases and updater channel. Require both public installers.
export function selectDesktopRelease(releases) {
  return releases
    .filter((r) => !r.draft && !r.prerelease && /^desktop-v\d+\.\d+\.\d+$/.test(r.tag_name))
    .sort((a, b) => {
      const av = a.tag_name.slice(9).split('.').map(Number);
      const bv = b.tag_name.slice(9).split('.').map(Number);
      return bv[0] - av[0] || bv[1] - av[1] || bv[2] - av[2];
    })
    .map((release) => {
      const version = release.tag_name.slice(9);
      const urls = ['aarch64', 'x86_64'].map((arch) => {
        const name = `kikimimi-desktop-${version}-${arch}.dmg`;
        const url = `${repository}/releases/download/${release.tag_name}/${name}`;
        return release.assets?.some((asset) => asset.name === name && asset.browser_download_url === url) ? url : null;
      });
      return urls.every(Boolean) ? { version, urls } : null;
    }).find(Boolean) ?? null;
}

export async function findDesktopRelease(fetcher = fetch) {
  // Paginate so frequent CLI releases cannot hide the desktop release.
  for (let page = 1; ; page++) {
    const response = await fetcher(`https://api.github.com/repos/isamisushi/kikimimi/releases?per_page=100&page=${page}`, {
      headers: { Accept: 'application/vnd.github+json' },
      signal: AbortSignal.timeout(8000),
    });
    if (!response.ok) throw new Error('Release lookup unavailable');
    const releases = await response.json();
    if (!Array.isArray(releases)) throw new Error('Invalid release list');
    const release = selectDesktopRelease(releases);
    if (release) return release;
    if (releases.length < 100) return null;
  }
}
