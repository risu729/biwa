import { defineConfig } from "cf/config";

// oxlint-disable-next-line import/no-default-export -- Configuration loaders require a default export.
export default defineConfig(({ isPreview }) => ({
	worker: {
		assets: { notFoundHandling: "single-page-application" },
		compatibilityDate: "2026-07-30",
		domains: isPreview ? [] : ["biwa.takuk.me"],
		name: "biwa-docs",
		observability: { logs: { enabled: true }, traces: { enabled: true } },
		previewUrls: true,
		workersDev: false,
	},
}));
