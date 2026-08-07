// Tauri ships no Node server, so the frontend is a pure SPA: adapter-static
// with an index.html fallback + `ssr = false` in src/routes/+layout.ts.
// https://v2.tauri.app/start/frontend/sveltekit/
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  compilerOptions: {
    // Force runes mode everywhere except node_modules (removable in Svelte 6).
    runes: ({ filename }) => (filename.split(/[/\\]/).includes("node_modules") ? undefined : true),
  },
  kit: {
    adapter: adapter({
      fallback: "index.html",
    }),
  },
};

export default config;
