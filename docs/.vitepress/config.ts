import { cloudflare } from "@cloudflare/vite-plugin";
import { defineConfig } from "vitepress";
import { withMermaid } from "vitepress-plugin-mermaid";

// https://vitepress.dev/reference/site-config
// oxlint-disable-next-line import/no-default-export
export default withMermaid(
	defineConfig({
		description: "CLI to execute commands on UNSW CSE servers from local",
		head: [["link", { href: "/icon.svg", rel: "icon" }]],
		srcDir: "src",
		themeConfig: {
			// https://vitepress.dev/reference/default-theme-config
			externalLinkIcon: true,
			footer: {
				copyright: 'Maintained by <a href="https://github.com/risu729">@risu729</a>',
				message:
					'Released under the <a href="https://github.com/risu729/biwa/blob/main/LICENSE">MIT License</a>.',
			},
			logo: "/icon.svg",
			nav: [
				{ link: "/", text: "Home" },
				{ link: "/about", text: "About" },
				{ link: "/getting-started", text: "Getting Started" },
				{ link: "/configuration", text: "Configuration" },
			],
			sidebar: [
				{
					items: [
						{ link: "/", text: "Overview" },
						{ link: "/about", text: "About" },
					],
					text: "Introduction",
				},
				{
					items: [
						{ link: "/getting-started", text: "Getting Started" },
						{ link: "/configuration", text: "Configuration" },
					],
					text: "Guide",
				},
				{
					items: [{ link: "/contributing", text: "Contributing" }],
					text: "Community",
				},
			],
			socialLinks: [{ icon: "github", link: "https://github.com/risu729/biwa" }],
		},
		title: "biwa",
		vite: {
			plugins: [
				// Cloudflare plugin doesn't work on dev for some reasons
				process.env.NODE_ENV === "production" ? cloudflare() : [],
			],
		},
	}),
);
