// Per-image solid-fill rectangles. {x,y,w,h} in pixels of the raw PNG.
// Manually filled after eyeballing the captured raws.
//
// Eyeball summary (1440x900 raws):
// - sources-list: UI hides tokens — clean, no fills.
// - notifiers-list: jellyfin api_key, telegram bot_token, telegram chat_id all
//   rendered in plaintext. Solid-black overlays cover them.
// - all other 8: clean, no fills.
export default {
  'runs-dashboard':    [],
  'live-run':          [],
  'flows-editor':      [],
  'browse-manual':     [],
  'plugins-browse':    [],
  'plugins-installed': [],
  'plugins-catalogs':  [],
  'sources-list':      [],
  'notifiers-list':    [
    { x: 695, y: 350, w: 300, h: 26, label: 'jellyfin_api_key' },
    { x: 695, y: 425, w: 380, h: 28, label: 'telegram_bot_token' },
    { x: 695, y: 450, w: 150, h: 26, label: 'telegram_chat_id' },
  ],
  'hardware-probe':    [],
};
