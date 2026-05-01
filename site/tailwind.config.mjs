/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{astro,html,md,js,ts,jsx,tsx}'],
  theme: {
    extend: {
      colors: {
        // Match crates/transcoderr's web/src/index.css palette.
        bg: '#0b0d0e',
        'bg-deep': '#08090a',
        surface: '#131618',
        'surface-2': '#181c1f',
        'surface-3': '#1f2326',
        border: '#232830',
        'border-strong': '#2f353d',
        text: '#ece9e1',
        // Marketing-page naming: `muted` = web app's --text-dim (visible dim),
        // `faint` = web app's --text-muted (deeper dim for labels).
        muted: '#9ca0a4',
        faint: '#686d72',
        accent: '#ffb627',
        'accent-soft': '#ffb62733',
        'accent-dim': '#c98712',
        'accent-line': '#ffb62755',
      },
      fontFamily: {
        // Web app uses JetBrains Mono for both body and mono.
        mono: ['"JetBrains Mono"', 'ui-monospace', 'SFMono-Regular', 'Menlo', 'Consolas', 'monospace'],
        sans: ['"JetBrains Mono"', 'ui-monospace', 'SFMono-Regular', 'monospace'],
      },
      maxWidth: { prose: '72ch' },
    },
  },
};
