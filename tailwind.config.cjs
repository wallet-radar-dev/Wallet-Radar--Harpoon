/** @type {import('tailwindcss').Config} */
module.exports = {
  // Scan the shell + the compiled bundle: every Tailwind class the app emits
  // appears as a literal string in app.ts/app.js (class maps like net.color,
  // meta.cls, ENTITY_TYPE_CLS) or index.html, so the JIT scanner finds them all.
  content: ["./web/index.html", "./web/app.ts", "./web/app.js"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        base: "#0b0f1a", panel: "#121826", panel2: "#1a2233",
        edge: "#27324a", accent: "#3b82f6", good: "#22c55e", bad: "#ef4444",
        warn: "#f59e0b", muted: "#64748b",
      },
      fontFamily: { mono: ["ui-monospace", "SFMono-Regular", "Menlo", "monospace"] },
    },
  },
  plugins: [],
};
