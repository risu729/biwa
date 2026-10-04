import { cloudflare } from "@cloudflare/vite-plugin";
import { defineConfig } from "vite";

// Package the fully rendered VitePress site as an assets-only Worker.
// oxlint-disable-next-line import/no-default-export -- Configuration loaders require a default export.
export default defineConfig({
	plugins: [cloudflare()],
	publicDir: ".vitepress/dist",
});
