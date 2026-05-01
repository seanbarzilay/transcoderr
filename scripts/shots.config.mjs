// Capture targets. Each entry is one PNG. URLs are relative to the
// server base. `viewport` is [width, height]. `wait` is a CSS selector
// to wait for before screenshotting; falsy = wait for networkidle only.
// `setup` names a hook in capture.mjs's `setupHooks` map; falsy = no hook.
// `tabClick` is a label text to click after navigation (for plugins page tabs).
export default [
  { name: 'runs-dashboard',    url: '/dashboard',  viewport: [1440, 900], wait: 'h2' },
  { name: 'live-run',          url: null,          viewport: [1440, 900], setup: 'rerunLatest' },
  { name: 'flows-editor',      url: '/flows/1',    viewport: [1600, 1000], wait: 'textarea, .cm-editor' },
  { name: 'browse-manual',     url: '/radarr',     viewport: [1440, 900], wait: 'h2' },
  { name: 'plugins-browse',    url: '/plugins',    viewport: [1440, 900], wait: '.plugin-tab', tabClick: 'Browse' },
  { name: 'plugins-installed', url: '/plugins',    viewport: [1440, 900], wait: '.plugin-tab' },
  { name: 'plugins-catalogs',  url: '/plugins',    viewport: [1440, 900], wait: '.plugin-tab', tabClick: 'Catalogs' },
  { name: 'sources-list',      url: '/sources',    viewport: [1440, 900], wait: 'h2' },
  { name: 'notifiers-list',    url: '/notifiers',  viewport: [1440, 900], wait: 'h2' },
  // hardware-probe: dropped — UI has no dedicated hardware page. Placeholder
  // PNG in site/public/screenshots/hardware-probe.png stays as-is for footer.
];
