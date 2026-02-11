import { defineConfig } from 'vitepress'
import { mermaid } from 'vitepress-plugin-mermaid'

// https://vitepress.dev/reference/site-config
export default defineConfig({
    title: "biwa",
    description: "CLI to execute commands on UNSW CSE servers from local",

    themeConfig: {
        // https://vitepress.dev/reference/default-theme-config
        nav: [
            { text: 'Home', link: '/' },
            { text: 'Getting Started', link: '/getting-started' },
            { text: 'Configuration', link: '/configuration' }
        ],

        sidebar: [
            {
                text: 'Introduction',
                items: [
                    { text: 'Overview', link: '/' },
                    { text: 'Comparison', link: '/comparison' }
                ]
            },
            {
                text: 'Guide',
                items: [
                    { text: 'Getting Started', link: '/getting-started' },
                    { text: 'Configuration', link: '/configuration' }
                ]
            }
        ],

        socialLinks: [
            { icon: 'github', link: 'https://github.com/risu729/biwa' }
        ]
    },

    // Enable mermaid plugin
    vite: {
        plugins: [
            mermaid({
                // Mermaid config options
            })
        ]
    }
})
