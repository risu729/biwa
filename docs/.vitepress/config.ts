import { defineConfig } from 'vitepress'
import { withMermaid } from 'vitepress-plugin-mermaid'

// https://vitepress.dev/reference/site-config
export default withMermaid(
    defineConfig({
        title: "biwa",
        description: "CLI to execute commands on UNSW CSE servers from local",
        srcDir: "src",

        themeConfig: {
            // https://vitepress.dev/reference/default-theme-config
            nav: [
                { text: 'Home', link: '/' },
                { text: 'About', link: '/about' },
                { text: 'Getting Started', link: '/getting-started' },
                { text: 'Configuration', link: '/configuration' }
            ],

            sidebar: [
                {
                    text: 'Introduction',
                    items: [
                        { text: 'Overview', link: '/' },
                        { text: 'About', link: '/about' }
                    ]
                },
                {
                    text: 'Guide',
                    items: [
                        { text: 'Getting Started', link: '/getting-started' },
                        { text: 'Configuration', link: '/configuration' },
                    ]
                },
                {
                    text: 'Community',
                    items: [
                        { text: 'Contributing', link: '/contributing' }
                    ]
                }
            ],

            socialLinks: [
                { icon: 'github', link: 'https://github.com/risu729/biwa' }
            ],

            footer: {
                message: 'Released under the [MIT License](https://github.com/risu729/biwa/blob/main/LICENSE).',
                copyright: 'Maintained by [@risu729](https://github.com/risu729)'
            }
        }
    })
)
