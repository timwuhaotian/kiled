# Kiled launch kit（曝光弹药库）

> 数据来源：README.md / DESIGN.md §1/§5.4，实测环境 M4 Pro + macOS 27.0 + rustc 1.94.0 release build。
> 严禁改数字；对外引用以此为准。Repo：https://github.com/timwuhaotian/kiled

## 真实数据速查（复制用）

- 体积：4.8 MB 下载 / 13.5 MB 安装后（单二进制 + 图标）
- 对比（Finder 实测，2026-10-21 最新 Apple Silicon 版）：TextMate 38.8 / BBEdit 68.8 / Sublime 146 / Nova 234 / Zed 269 / VS Code 920 / Cursor 1039 MB
- 空闲 CPU：0.00%（光标不闪烁）/ 0.83%（闪烁）
- 按键延迟：0.14–0.33 ms 平均（51k 行文件 0.14–0.15 ms；2.3k 行 0.28–0.33 ms；最坏约 1.5 ms）
- 首帧 ~126 ms；打开 51k 行 Rust 文件 96 ms；单文件约 111 MB RSS
- 技术：Rust + GPUI 0.2.2（Metal）+ tree-sitter；LSP 只用已安装的 server，不多装东西
- 语言/LSP：Rust（rust-analyzer）、TS/JS（TS7 `tsc --lsp`，≤6 回退 typescript-language-server）、JSON、YAML、TOML（taplo）
- 按键：VS Code 默认键位，可重绑（Settings ▸ Keyboard / `keymap.toml`）
- 五个主题与 Kil 终端同名同色：Night / Paper / Fjord / Ember / Dusk
- 定位：只写代码，无终端、无插件、无 git 面板、无分屏；macOS 14+ Apple Silicon only；MIT；Developer ID 签名 + Apple 公证；静默自动更新

## 一句话差异化

- vs VS Code：按键不用学，体积是它的 1/68（13.5 MB vs 920 MB），空闲 CPU 0%。
- vs Zed：体积是它的 1/20（13.5 MB vs 269 MB），不登录、不联网协作，只做本地编辑器。
- vs Sublime：开源 MIT + 原生 LSP（diagnostics/completion/hover/definition/format）开箱即用，不用配插件。

---

## HN：Show HN 草稿（英文）

Title: `Show HN: Kiled – a 13.5MB native macOS code editor in Rust`

Body:

```
Hi HN. Kiled is a native macOS code editor I wrote in Rust. It is the
sibling of my Kil terminal: same GPUI/Metal stack, same five themes.

It edits code and does nothing else: no terminal, no extensions, no git
panel, no split views. VS Code's keys are the defaults, so existing
muscle memory works on day one (⌘P, ⌘D, ⌥↑↓, F12, F8, ⇧⌥F…), and every
binding can be rebound in the app.

What it does: tree-sitter highlighting (Rust, TS/JS, JSON, YAML, TOML),
LSP via the servers you already have (rust-analyzer, TS7's own server,
vscode-json, yaml-language-server, taplo), multi-cursor, find/replace,
Quick Open, explorer with rename/trash, atomic saves, session restore,
quiet auto-update (signed + notarized).

Measured numbers (M4 Pro, release build, method in DESIGN.md §5.4):
- 4.8 MB download, 13.5 MB installed (VS Code 920 MB, Zed 269 MB,
  Sublime 146 MB, measured the same way in Finder)
- Idle CPU 0.00% (0.83% with cursor blinking)
- Keystroke 0.14–0.33 ms average on a 51k-line file, worst key ~1.5 ms
- First frame ~126 ms after launch

macOS 14+, Apple silicon only, MIT. Early status (0.0.x): single
window, UTF-8 only, no find-in-files yet. Non-goals are listed honestly
in DESIGN.md §1.

Repo: https://github.com/timwuhaotian/kiled

Happy to answer questions about GPUI, tree-sitter incremental parsing,
or keeping LSP child processes on a leash (each server gets its own
process group; nothing outlives the editor, kill -9 included).
```

回帖准备（只回真实有的）：
- 为什么不用 Zed 的 editor crate？GPL-3.0，且超出需求；行为按 VS Code 文档行为重写。
- 数据怎么测的？DESIGN.md §5.4 + `cargo test --release -p kiled-core --test latency`。
- Windows/Linux？不做，macOS only 是设计选择。

## Reddit r/macapps 草稿

Title: `Kiled – a tiny (13.5 MB) native Mac code editor with VS Code keys [MIT, Apple silicon]`

```
If you like VS Code's keys but not its 920 MB footprint: Kiled is a
native macOS editor (Rust + Metal) that installs at 13.5 MB, idles at
0% CPU, and opens in ~126 ms.

- VS Code keymap by default, rebindable in Settings
- Syntax highlighting + LSP (errors, completion, hover, go-to-definition,
  format) for Rust, TypeScript/JavaScript, JSON, YAML, TOML
- Multi-cursor, Quick Open (⌘P), Command Palette, Go to Line
- File explorer with rename/move-to-trash, session restore, atomic saves
- 5 themes shared with its sibling Kil terminal
- Signed + notarized, quiet auto-update, MIT licensed

Deliberately small: no terminal, no extensions, no git panel, one
window. macOS 14+ on Apple silicon.

https://github.com/timwuhaotian/kiled (screenshot + size chart in README)

Feedback welcome — it's early (0.0.x), and the roadmap/non-goals are in
DESIGN.md.
```

## Reddit r/rust 草稿

Title: `Kiled: a native macOS code editor in Rust – 13.5 MB, 0% idle CPU, sub-ms keystrokes`

```
Kiled is my Rust code editor for macOS (sibling of the Kil terminal),
built on GPUI 0.2.2 (vendored, Metal) + tree-sitter + ropey, with its own
minimal LSP client (12 messages, no lsp-types).

Some Rust-side details:
- Core (`kiled-core`) is UI-free and unit-tested; the only `unsafe` is
  three Darwin syscalls (killpg, waitid, fcopyfile) with SAFETY comments.
- One transaction per edit: rope + incremental reparse + diagnostics +
  LSP log move together.
- Each LSP server runs in its own process group; quit/kill -9 leaves
  nothing behind (verified with tests + a fake shell server).
- Budgets, measured on M4 Pro: idle 0.00%, keystroke 0.14–0.33 ms avg
  on 51k lines, first frame ~126 ms. Method + test commands in DESIGN.md.

MIT, Apple silicon, macOS 14+. Early but usable daily for
Rust/TS/JSON/YAML/TOML.

https://github.com/timwuhaotian/kiled
```

## X / Twitter 短帖 ×3

1. `Kiled: a native macOS code editor in Rust. 4.8MB download, 13.5MB installed (VS Code: 920MB). VS Code keys, tree-sitter + LSP, 0% idle CPU, MIT. https://github.com/timwuhaotian/kiled`
2. `What I wanted: VS Code's keys without the 920MB. So I wrote Kiled — Rust + Metal, sub-ms keystrokes on 51k-line files, first frame ~126ms, auto-updates quietly. macOS 14+ / Apple silicon / MIT. https://github.com/timwuhaotian/kiled`
3. `Kiled is Kil's sibling: same Rust/GPUI stack, same 5 themes — one for terminal, one for code. No extensions, no git panel, no bloat. Just editing. https://github.com/timwuhaotian/kiled`

配图建议：README 截图 + size.svg 二选一；带 `#rust #macos` 标签。

## V2EX 草稿（中文）

标题：`[分享] 写了个 macOS 原生代码编辑器 Kiled：Rust 写，13.5MB，VS Code 按键`

正文：

```
Kiled，一个 macOS 原生的代码编辑器，Rust + GPUI(Metal) + tree-sitter，
是我之前写的 Kil 终端的 sibling 项目，同栈同主题。

定位很窄：只写代码，没有终端、没有插件、没有 git 面板、没有分屏。
按键默认就是 VS Code 那套（⌘P/⌘D/⌥↑↓/F12/F8/⇧⌥F…），可在应用内重绑。

功能（0.0.x，早期但可日常用）：
- 高亮：Rust、TS/JS（含 TSX/JSX）、JSON、YAML、TOML
- LSP：用你本机已装的 server（rust-analyzer、TS7 自带 server、
  vscode-json、yaml-language-server、taplo），报错下划线、补全、
  hover、跳转定义、格式化；没装会告诉你怎么装
- 多光标、文件内查找替换、Quick Open、命令面板、资源管理器
  （新建/重命名/废纸篓）、原子保存、会话恢复、静默自动更新

实测数据（M4 Pro，方法见 DESIGN.md §5.4）：
- 下载 4.8MB，安装 13.5MB（同口径：VS Code 920MB，Zed 269MB，
  Sublime 146MB）
- 空闲 CPU 0.00%，按键 0.14–0.33ms（5 万行文件），首帧约 126ms

macOS 14+，仅 Apple Silicon，MIT，签名+公证。
README 里有截图和体积对比图，非目标写在 DESIGN.md §1 里，没画饼。

https://github.com/timwuhaotian/kiled
欢迎试用拍砖。
```

## 掘金草稿（中文）

标题：`13.5MB 的 macOS 原生编辑器：我用 Rust 写了个只干一件事的代码编辑器`

正文：

```
# 为什么写 Kiled

VS Code 好用，但 920MB + 常驻进程让我不适；Zed 269MB 且重心在协作；
Sublime 闭源、LSP 靠配插件。我想要的东西很小：VS Code 的按键、
原生速度、开箱即用的 LSP，其余都不要。于是写了 Kiled——
Kil 终端的 sibling 项目，同样的 Rust + GPUI + tree-sitter 栈。

# 它是什么

macOS 原生代码编辑器（Metal 渲染），只做编辑：
无终端、无插件、无 git 面板、无分屏、单窗口。
macOS 14+，仅 Apple Silicon，MIT 开源，签名+公证，静默自动更新。

# 功能

- **VS Code 按键**：⌘P/⇧⌘P/⌘D/⌥↑↓/⌘//F12/F8/⇧⌥F…全部默认一致，
  应用内可视化重绑，存 `~/.config/kiled/keymap.toml`
- **高亮**：tree-sitter，Rust/TS/JS/JSON/YAML/TOML，增量解析、只渲染可视行
- **LSP**：复用本机 server，不重复造轮子；diagnostics、补全、hover、
  跳转定义、格式化；缺 server 时提示安装命令
- **编辑基本功**：多光标、查找替换、Quick Open（尊重 .gitignore）、
  命令面板、Go to Line、前进后退
- **文件安全**：原子保存（保留权限/ACL/扩展属性/符号链接/换行符）、
  脏文件关闭必问、磁盘变更感知、Revert File 可撤销
- **体验**：会话恢复、5 套与 Kil 同名主题、状态栏显示 server 状态

# 性能（实测，非口号）

M4 Pro + release 构建，方法与命令公开在 DESIGN.md §5.4：

| 指标 | 数据 |
|---|---|
| 下载 / 安装 | 4.8MB / 13.5MB（VS Code 920MB，Zed 269MB） |
| 空闲 CPU | 0.00%（光标闪烁 0.83%） |
| 按键延迟 | 5 万行文件平均 0.14–0.15ms |
| 首帧 | 约 126ms |

# 工程上比较有意思的点

- 每次编辑是一个事务：rope + 增量重解析 + 诊断 + LSP 日志一起走，
  不可能互相不一致
- 每个 LSP server 独立进程组，退出/kill -9 不留孤儿进程
  （连 server 拉起的子进程一起杀）
- 全仓 `unsafe` 只有三处 Darwin 调用，都有 SAFETY 注释

# 诚实声明

0.0.x 早期版本：单窗口、仅 UTF-8、无文件内全局搜索、无 rename symbol。
非目标清单在 DESIGN.md §1，不画饼。

Repo：https://github.com/timwuhaotian/kiled
觉得方向对的求个 star，提 issue 更欢迎。
```

---

## 30 天发布节奏

| 天 | 动作 | 渠道 | 备注 |
|---|---|---|---|
| D1 | 发 Show HN（周二–周四上午美西时间） | HN | 当天只守评论，不群发别处 |
| D2 | 按 HN 反馈修 README（截图/FAQ） | 仓库 | 把被追问 2 次以上的写进 README |
| D4 | 发 r/rust | Reddit | 技术向，突出 unsafe/LSP 进程组/测试 |
| D7 | 发 r/macapps | Reddit | 用户向，突出体积/按键/截图 |
| D8 | X 短帖 #1（体积对比） | X | 配 size.svg |
| D10 | V2EX 分享帖 | V2EX | 晚 8–10 点发，守评论 2 小时 |
| D12 | X 短帖 #2（按键+延迟） | X | 配编辑器截图 |
| D14 | 掘金长文 | 掘金 | 周末发，带性能表 |
| D18 | X 短帖 #3（Kil sibling 联动） | X | @/引用 Kil 帖子 |
| D21 | 复盘：star/issue/流量来源 | 内部 | 决定下一轮选题（性能深挖 or LSP 篇） |
| D25–30 | 第二轮：-technical deep-dive（LSP 进程组 / 按键事务二选一） | 掘金+Reddit | 用 D21 数据定题，别重复发同一内容 |

规则：同一内容不一天多发；每次发完 24h 内回完评论；所有数字只减不加（版本迭代后以新实测为准）。
