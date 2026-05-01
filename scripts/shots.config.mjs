// Capture targets. Each entry is one PNG. URLs are relative to the
// server base. `viewport` is [width, height]. `wait` is a CSS selector
// to wait for before screenshotting; falsy = wait for networkidle only.
// `setup` names a hook in capture.mjs's `setupHooks` map; falsy = no hook.
export default [
  { name: 'runs-dashboard',    url: '/',                       viewport: [1440, 900], wait: 'main' },
  { name: 'live-run',          url: '/runs/CURRENT',           viewport: [1440, 900], setup: 'triggerTranscode' },
  { name: 'flows-editor',      url: '/flows/CURRENT/edit',     viewport: [1600, 1000], wait: 'textarea, .cm-editor' },
  { name: 'browse-manual',     url: '/browse/CURRENT',         viewport: [1440, 900], wait: 'main' },
  { name: 'plugins-browse',    url: '/plugins#browse',         viewport: [1440, 900], wait: 'main' },
  { name: 'plugins-installed', url: '/plugins#installed',      viewport: [1440, 900], wait: 'main' },
  { name: 'plugins-catalogs',  url: '/plugins#catalogs',       viewport: [1440, 900], wait: 'main' },
  { name: 'sources-list',      url: '/settings/sources',       viewport: [1440, 900], wait: 'main' },
  { name: 'notifiers-list',    url: '/settings/notifiers',     viewport: [1440, 900], wait: 'main' },
  { name: 'hardware-probe',    url: '/settings/hw',            viewport: [1440, 900], wait: 'main' },
];
