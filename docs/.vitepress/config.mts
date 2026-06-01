import { defineConfig } from "vitepress";

export default defineConfig({
  title: "OxiCloud",
  description: "Self-hosted cloud storage, calendar & contacts — blazingly fast",

  base: "/OxiCloud/",

  sitemap: {
    hostname: "https://diocrafts.github.io/OxiCloud",
    lastmodDateOnly: false,
  },

  // Internal planning notes (docs/plan/*) and AI hand-off prompts
  // aren't user-facing docs and contain raw Rust/pseudocode whose
  // generics (Option<String>, Vec<SubjectGroup>, …) trip Vue's
  // template parser ("Element is missing end tag" because <String>,
  // <SubjectGroup>, etc. look like unclosed HTML tags). Keep them
  // in-tree for engineering reference but skip them at site-build time.
  srcExclude: [
    "plan/**",
    "**/*.prompt",
  ],

  markdown: {
    image: {
      lazyLoading: true,
    },
  },

  lastUpdated: true,

  ignoreDeadLinks: [
    /^https?:\/\/localhost/,
  ],

  locales: {
    root: {
      label: "English",
      lang: "en",
    },
  },

  head: [
    ["link", { rel: "icon", href: "/OxiCloud/logo.svg" }],
  ],

  themeConfig: {
    logo: "/logo.svg",

    search: {
      provider: "local",
    },

    editLink: {
      pattern: "https://github.com/DioCrafts/OxiCloud/tree/main/docs/:path",
      text: "Edit this page on GitHub",
    },

    nav: [
      { text: "Home", link: "/" },
      { text: "Guide", link: "/guide/" },
      { text: "Configuration", link: "/config/" },
      { text: "FAQ", link: "/faq" },
    ],

    sidebar: {
      "/": [
        {
          text: "Introduction",
          items: [
            { text: "What is OxiCloud?", link: "/guide/" },
            { text: "Quick Start", link: "/guide/installation" },
          ],
        },
        {
          text: "Configuration",
          items: [
            { text: "Deployment & Docker", link: "/config/deployment" },
            { text: "Environment Variables", link: "/config/env" },
            { text: "Authentication", link: "/config/authentication" },
            { text: "OIDC / SSO", link: "/config/oidc" },
            { text: "OIDC Config Examples", link: "/config/oidc-config-examples" },
            { text: "Admin Settings", link: "/config/admin-settings" },
            { text: "WOPI (Office Editing)", link: "/config/wopi" },
          ],
        },
        {
          text: "Features",
          items: [
            { text: "WebDAV", link: "/guide/webdav" },
            { text: "CalDAV & CardDAV", link: "/guide/caldav-carddav" },
            { text: "DAV Client Setup", link: "/guide/dav-client-setup" },
            { text: "Chunked Uploads", link: "/guide/chunked-uploads" },
            { text: "Batch Operations", link: "/guide/batch-operations" },
            { text: "Deduplication", link: "/guide/deduplication" },
            { text: "Favorites & Recent", link: "/guide/favorites-and-recent" },
            { text: "Search", link: "/guide/search" },
            { text: "Thumbnails & Transcoding", link: "/guide/thumbnails-and-transcoding" },
            { text: "Trash & Recycle Bin", link: "/guide/trash" },
            { text: "ZIP & Compression", link: "/guide/zip-and-compression" },
            { text: "Internationalization", link: "/guide/i18n" },
          ],
        },
        {
          text: "Architecture",
          items: [
            { text: "Internal Architecture", link: "/architecture/" },
            { text: "Caching", link: "/architecture/caching" },
            { text: "Resource Listing API", link: "/architecture/resource-listing" },
            { text: "Storage Safety", link: "/architecture/file-system-safety" },
            { text: "Database Transactions", link: "/architecture/database-transactions" },
            { text: "Share Integration", link: "/architecture/share-integration" },
            { text: "Storage Quotas", link: "/architecture/storage-quotas" },
            { text: "File and Blob lifecycle", link: "/architecture/file-and-blob-lifecycle" },
            { text: "ReBAC & Authorization", link: "/architecture/rebac-authorization" },
          ],
        },
        { text: "FAQ", link: "/faq" },
      ],
    },

    socialLinks: [
      { icon: "github", link: "https://github.com/DioCrafts/OxiCloud" },
    ],

    footer: {
      message: "Released under the MIT License.",
      copyright: "Copyright © 2025-present DioCrafts",
    },
  },
});
