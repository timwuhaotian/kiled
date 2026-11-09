# Kiled 官网 SEO 技术规格

> 结论：先上子路径（零成本、继承主站权重），有流量后再迁 `kiled.dev`。全站静态 HTML、无框架，与 Kiled 的小体积人设一致。以下所有数字来自 `README.md` / `DESIGN.md §1/§5.4`，未编造。

数据快照（写文案只用这些）：4.8 MB 下载 / 13.5 MB 安装；原生 macOS、Rust + GPUI(Metal) + tree-sitter；VS Code 按键；LSP：rust-analyzer、TypeScript 7 `tsc --lsp`、vscode-json-language-server、yaml-language-server、taplo；idle 0.00% CPU、按键 0.14–0.33 ms、首帧 ~126 ms（M4 Pro, macOS 27, release）；自动更新（SHA-256 + Developer ID + notarization）；MIT；macOS 14+ Apple Silicon only；Kil 终端的 sibling；版本 0.0.9（2026-11-09）；仓库 https://github.com/timwuhaotian/kiled。

## 1. URL 推荐

- **起步（推荐）：个人站子路径**，如 `https://<you>.dev/kiled/`。零成本、立刻上线、继承主域权重；GitHub Pages 备选 `https://timwuhaotian.github.io/kiled/`。
- **有自然流量后：独立域 `https://kiled.dev/`**，子路径 301 全量跳转到新域对应路径，权重传递约 3 个月。
- **不要**：`kiled.github.io` 新组织站（权重从零且与个人品牌割裂）、Hash 路由 SPA（不可抓取）。
- 规范：全站小写 + 尾斜杠统一（选一种 301 到另一种）；`www` 301 到裸域；全站 HTTPS + HSTS；canonical 自指。

## 2. 页面清单（4 页起步，不多做）

| 路径 | 目的 | 抓取优先级 |
|---|---|---|
| `/kiled/` | 首页：转化下载，承接 "kiled" / "macos code editor" | 1.0，每周更新 |
| `/kiled/download/` | 下载页：DMG 直链 + 校验 + LSP 安装表，承接 "kiled download" | 0.9 |
| `/kiled/vs/` | 对比页：size.svg 全量对比表，承接 "vs code alternative mac" 等长尾 | 0.8 |
| `/kiled/changelog/` | 更新日志：CHANGELOG.md 同步，抓取新鲜度信号 | 0.6 |

后续再加（现在不做）：`/kiled/docs/`（keymap/config）、`/kiled/press/`（icon/screenshot 原图包）。

## 3. 每页 Title / Description / H1

说明：title ≤ 60 字符、description 150–160 字符，均英文（产品语言）；H1 每页唯一且含主关键词。

### 3.1 首页 `/kiled/`

```html
<title>Kiled — Native macOS Code Editor, 13.5 MB, VS Code Keys</title>
<meta name="description" content="Kiled is a native macOS code editor in Rust: 4.8 MB download, VS Code keys, LSP for Rust/TS/JSON/YAML/TOML, auto-updates. Free and MIT-licensed.">
<link rel="canonical" href="https://<you>.dev/kiled/">
<h1>Kiled: a native macOS code editor in 13.5 MB</h1>
```

正文必须覆盖的关键词（自然出现一次即可）：native macOS code editor、lightweight、Rust、tree-sitter、rust-analyzer、TypeScript、Apple Silicon。

### 3.2 下载页 `/kiled/download/`

```html
<title>Download Kiled for macOS (Apple Silicon) — 4.8 MB DMG</title>
<meta name="description" content="Download Kiled-arm64.dmg (4.8 MB) for macOS 14+ on Apple Silicon. Signed, notarized, verifies SHA-256, and updates itself quietly.">
<link rel="canonical" href="https://<you>.dev/kiled/download/">
<h1>Download Kiled for macOS</h1>
```

页内要素：DMG 直链（`https://raw.githubusercontent.com/timwuhaotian/kiled/main/releases/Kiled-arm64.dmg`）+ `.sha256` 校验命令 + 系统要求（macOS 14+, Apple Silicon）+ LSP 安装表（与 README 一致）。

### 3.3 对比页 `/kiled/vs/`

```html
<title>Kiled vs VS Code, Zed, Sublime: Installed Size Compared</title>
<meta name="description" content="Kiled is 13.5 MB installed vs VS Code 920 MB, Zed 269 MB, Sublime 146 MB. Same-method Finder sizes, Apple Silicon builds, measured 21 Oct 2026.">
<link rel="canonical" href="https://<you>.dev/kiled/vs/">
<h1>Kiled vs the editors you might be using</h1>
```

页内要素：把 README §Size 的 8 个数字做成 HTML `<table>`（不要只有图片，爬虫读不到 svg 文字）+ 测量方法脚注（Finder、ARM64 下载、universal 去 Intel 后 135/86/47/28 MB）。

### 3.4 更新日志 `/kiled/changelog/`

```html
<title>Kiled Changelog — Latest Releases</title>
<meta name="description" content="Kiled changelog: every release of the 13.5 MB native macOS code editor. Current version 0.0.9 — right-click menus, VS Code editing fixes.">
<link rel="canonical" href="https://<you>.dev/kiled/changelog/">
<h1>Kiled changelog</h1>
```

说明：每次发版同步 `CHANGELOG.md`；description 中的版本号随发版更新。

## 4. JSON-LD 模板（直接粘贴）

说明：首页放 `SoftwareApplication`，首页 FAQ 区放 `FAQPage`（页面必须有肉眼可见的同款问答文字，否则算作弊）。`<you>.dev` 换成真实域名。

### 4.1 SoftwareApplication（首页 `<head>`）

```html
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  "name": "Kiled",
  "applicationCategory": "DeveloperApplication",
  "operatingSystem": "macOS 14 or later",
  "processorRequirements": "Apple Silicon",
  "softwareVersion": "0.0.9",
  "description": "A native macOS code editor written in Rust (13.5 MB installed, 4.8 MB download). VS Code keys, tree-sitter highlighting, LSP for Rust, TypeScript, JSON, YAML and TOML, quiet auto-updates.",
  "url": "https://<you>.dev/kiled/",
  "downloadUrl": "https://raw.githubusercontent.com/timwuhaotian/kiled/main/releases/Kiled-arm64.dmg",
  "codeRepository": "https://github.com/timwuhaotian/kiled",
  "author": { "@type": "Person", "name": "timwuhaotian", "url": "https://github.com/timwuhaotian" },
  "license": "https://github.com/timwuhaotian/kiled/blob/main/LICENSE",
  "isAccessibleForFree": true,
  "offers": { "@type": "Offer", "price": "0", "priceCurrency": "USD" },
  "fileSize": "4.8 MB",
  "screenshot": "https://<you>.dev/kiled/og/screenshot-1200.png"
}
</script>
```

### 4.2 FAQPage（首页 FAQ 区下方）

```html
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "FAQPage",
  "mainEntity": [
    {
      "@type": "Question",
      "name": "How big is Kiled?",
      "acceptedAnswer": { "@type": "Answer", "text": "4.8 MB download, 13.5 MB installed — one binary and its icon, versus 920 MB for VS Code and 269 MB for Zed measured the same way." }
    },
    {
      "@type": "Question",
      "name": "Which languages does Kiled support?",
      "acceptedAnswer": { "@type": "Answer", "text": "Syntax highlighting for Rust, TypeScript, TSX, JavaScript, JSX, JSON, JSONC, YAML and TOML, with language servers: rust-analyzer, TypeScript 7's own server, vscode-json-language-server, yaml-language-server and taplo." }
    },
    {
      "@type": "Question",
      "name": "Does Kiled use VS Code keyboard shortcuts?",
      "acceptedAnswer": { "@type": "Answer", "text": "Yes. Cmd+P, Shift+Cmd+P, Cmd+D, F12, F8 and the rest are VS Code's macOS defaults, and every command can be rebound in Settings or keymap.toml." }
    },
    {
      "@type": "Question",
      "name": "Which Macs does Kiled run on?",
      "acceptedAnswer": { "@type": "Answer", "text": "Apple Silicon Macs running macOS 14 or later. There are no Intel builds." }
    },
    {
      "@type": "Question",
      "name": "How do updates work?",
      "acceptedAnswer": { "@type": "Answer", "text": "Kiled checks for a new release at launch and every six hours, verifies SHA-256, Developer ID signature and Apple notarization, and installs it when you quit." }
    },
    {
      "@type": "Question",
      "name": "Is Kiled free and open source?",
      "acceptedAnswer": { "@type": "Answer", "text": "Yes. Kiled is MIT-licensed and free; the source is on GitHub." }
    }
  ]
}
</script>
```

## 5. OG / Twitter 卡片规范

说明：复用现有素材，不新增设计。用 `docs/screenshot.png` 裁 1200×630，用 `docs/icon.png` 做方形 logo。

```html
<!-- 每页通用（换 title/description/url 为本页值） -->
<meta property="og:type" content="website">
<meta property="og:site_name" content="Kiled">
<meta property="og:title" content="Kiled — Native macOS Code Editor, 13.5 MB">
<meta property="og:description" content="4.8 MB download. VS Code keys, LSP, quiet auto-updates. Free and MIT-licensed.">
<meta property="og:url" content="https://<you>.dev/kiled/">
<meta property="og:image" content="https://<you>.dev/kiled/og/screenshot-1200.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:image:alt" content="Kiled editor: explorer, three tabs, TypeScript with errors underlined">
<meta name="twitter:card" content="summary_large_image">
```

素材清单（`/kiled/og/` 下）：`screenshot-1200.png`（1200×630, ≤300 KB）、`icon-512.png`（现有 icon 直用）。上线前用 [OpenGraph.xyz](https://www.opengraph.xyz/) 或 iMessage/Telegram 实测一次。

## 6. robots / sitemap 规范

说明：默认全放行，并**显式放行 AI 爬虫**（PerplexityBot/GPTBot/ClaudeBot），让 AI 搜索能引用 Kiled。不屏蔽任何 AI 训练爬虫——早期项目要的是曝光。

### 6.1 robots.txt（`/<root>/robots.txt`，子路径方案则放主站根）

```txt
User-agent: *
Allow: /

# AI search & assistants: explicitly allowed
User-agent: PerplexityBot
Allow: /

User-agent: GPTBot
Allow: /

User-agent: ChatGPT-User
Allow: /

User-agent: OAI-SearchBot
Allow: /

User-agent: ClaudeBot
Allow: /

User-agent: anthropic-ai
Allow: /

User-agent: Google-Extended
Allow: /

Sitemap: https://<you>.dev/kiled/sitemap.xml
```

### 6.2 sitemap.xml（`/kiled/sitemap.xml`，4 页）

```xml
<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://<you>.dev/kiled/</loc><changefreq>weekly</changefreq><priority>1.0</priority></url>
  <url><loc>https://<you>.dev/kiled/download/</loc><changefreq>monthly</changefreq><priority>0.9</priority></url>
  <url><loc>https://<you>.dev/kiled/vs/</loc><changefreq>monthly</changefreq><priority>0.8</priority></url>
  <url><loc>https://<you>.dev/kiled/changelog/</loc><changefreq>weekly</changefreq><priority>0.6</priority></url>
</urlset>
```

上线动作：Search Console + Bing Webmaster 提交 sitemap；发版后用 Search Console 的 URL 检查请求重新抓取 changelog 页。

## 7. Core Web Vitals 预算

说明：静态站应轻松全绿；预算按"编辑器本体很小，官网也得小"定。

| 指标 | 预算（移动 4G, Moto G4） | 做法 |
|---|---|---|
| LCP | ≤ 1.5 s（Google 线 2.5 s） | 纯静态 HTML，无 JS 框架；首屏图 `fetchpriority="high"` + AVIF/WebP；无 webfont（系统字体栈 `-apple-system`） |
| INP | ≤ 100 ms（线 200 ms） | 首屏零 JS；全站 JS ≤ 10 KB（仅移动端菜单/复制按钮），`defer` |
| CLS | ≤ 0.05（线 0.1） | 所有图片写 `width`/`height`；无顶部横幅位移 |
| 整页重量 | 首页 ≤ 300 KB（含图） | screenshot 压到 ≤ 200 KB；size 对比用 HTML 表格 + 内联 SVG |
| 可访问性底线 | Lighthouse ≥ 95 | `lang="en"`、按钮可聚焦、颜色对比 ≥ 4.5:1、图片全 `alt` |

验收：每个页面跑一次 PageSpeed Insights（移动），三项 CWV 全绿才算上线完成；把截图存 `docs/` 备查。
