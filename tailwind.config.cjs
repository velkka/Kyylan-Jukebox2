/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ['./src/renderer/**/*.{html,ts,tsx}'],
  theme: {
    extend: {
      colors: {
        // Kyylan Jukebox accent palette
        jukebox: {
          bg: '#0e0b14',
          panel: '#171221',
          accent: '#c04cff',
          accent2: '#4cc0ff'
        }
      }
    }
  },
  plugins: []
}
