/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./src/templates/**/*.html",
  ],
  theme: {
    extend: {
      colors: {
        'nix-blue': '#7EBAE4',
        'nix-dark': '#5277C3',
      }
    },
  },
  plugins: [],
}