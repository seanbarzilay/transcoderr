/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{astro,html,md,js,ts,jsx,tsx}'],
  theme: {
    extend: {
      colors: {
        bg: '#0b0d10',
        surface: '#14171c',
        'surface-2': '#1c2027',
        accent: '#7dd3fc',
        muted: '#94a3b8',
        text: '#e5e7eb',
      },
      fontFamily: {
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'Consolas', 'monospace'],
        sans: ['system-ui', '-apple-system', 'Segoe UI', 'Roboto', 'sans-serif'],
      },
      maxWidth: { prose: '72ch' },
    },
  },
};
