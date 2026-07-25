import type * as Preset from '@docusaurus/preset-classic';
import type {Config} from '@docusaurus/types';
import {themes as prismThemes} from 'prism-react-renderer';

const url = 'https://spate-benchmark.kainth.dev';
const githubUrl = 'https://github.com/MarcusKainth/spate-benchmark';

const config: Config = {
  title: 'Spate Benchmark',
  tagline: 'Streaming ETL systems on one fixed pipeline: Kafka → Avro → ClickHouse',
  favicon: 'img/favicon.svg',

  url,
  baseUrl: '/',
  trailingSlash: false,

  organizationName: 'MarcusKainth',
  projectName: 'spate-benchmark',

  // A benchmark that silently drops a link to its own methodology is worse than
  // one that fails to build. Every one of these is a hard error on purpose.
  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },

  future: {
    // Rspack + SWC + Lightning CSS in place of webpack, Babel and Terser.
    faster: true,
    v4: {
      // Required by `faster`: SSG runs in worker threads, which cannot support
      // the legacy post-build head attribute.
      removeLegacyPostBuildHeadAttribute: true,
    },
  },

  i18n: {defaultLocale: 'en', locales: ['en']},

  plugins: ['./plugins/bench-data'],

  presets: [
    [
      'classic',
      {
        docs: {
          path: '../docs',
          routeBasePath: '/',
          sidebarPath: './sidebars.ts',
          editUrl: `${githubUrl}/edit/main/docs/`,
          showLastUpdateTime: true,
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {defaultMode: 'dark', respectPrefersColorScheme: true},
    navbar: {
      title: 'Spate Benchmark',
      items: [
        {to: '/', label: 'Results', position: 'left', activeBaseRegex: '^/$'},
        {to: '/methodology', label: 'Methodology', position: 'left'},
        {to: '/environments', label: 'Environments', position: 'left'},
        {to: '/limitations', label: 'Limitations', position: 'left'},
        {href: githubUrl, label: 'GitHub', position: 'right'},
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'The benchmark',
          items: [
            {label: 'Results', to: '/'},
            {label: 'Methodology', to: '/methodology'},
            {label: 'Limitations', to: '/limitations'},
            {label: 'Reproducing this', to: '/reproduce'},
          ],
        },
        {
          title: 'Detail',
          items: [
            {label: 'The workload', to: '/workload'},
            {label: 'Environments', to: '/environments'},
            {label: 'Roadmap', to: '/roadmap'},
          ],
        },
        {
          title: 'Source',
          items: [
            {label: 'Repository', href: githubUrl},
            {label: 'Spate', href: 'https://github.com/MarcusKainth/spate-etl'},
          ],
        },
      ],
      copyright:
        'Code Apache-2.0. Published results CC-BY-4.0 — use them, cite them, and please link back so a reader can check the provenance.',
    },
    prism: {theme: prismThemes.github, darkTheme: prismThemes.dracula},
  } satisfies Preset.ThemeConfig,
};

export default config;
