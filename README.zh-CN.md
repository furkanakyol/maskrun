[English](README.md) · **简体中文** · [Español](README.es.md) · [Português (BR)](README.pt-BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Türkçe](README.tr.md)

> 本文是英文 README 的翻译。若有出入，以[英文版](README.md)为准。

# maskrun

用操作系统钥匙串里的密钥来运行命令 —— 并让这些值始终待在 AI 编码代理的上下文之外。

```bash
maskrun put myapp-database-url        # 存进钥匙串，绝不落盘
maskrun run -- npm run dev            # 注入子进程，输出中被遮蔽
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

单个二进制文件，无运行时依赖，无守护进程。支持 Linux、macOS 和 Windows。

---

## 问题

把密钥从 `.env` 文件搬进操作系统钥匙串，是容易的那一半。难的那一半，要等到 AI
代理开始操作你的 shell 时才浮现。

把密钥注入子进程，能挡住代理**读取保险库**。但它对**从进程里回流出来**的值毫无
办法：

- 开发服务器启动时打印出连接串
- `curl -v` 把 `Authorization` 头原样回显
- 一条堆栈跟踪带着 DSN
- `psql` 引用了它连不上的那个 URL

其中任何一种，都会把密钥送进对话记录；从那一刻起，它就成了对话的一部分、终端回滚
缓冲的一部分，以及那份记录被存放到的任何地方的一部分。

maskrun 堵上这条路，以及它旁边的那几条。

## 安装

**Linux / macOS** —— 下载预编译二进制文件，校验其 checksum，不需要编译器：

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows（PowerShell）**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**通过 Rust 工具链：**

```bash
cargo install maskrun
```

**从源码构建：**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# 二进制文件位于 target/release/maskrun
```

## 快速上手

```bash
# 1. 把现有的 .env 搬进钥匙串 —— 先看清楚再动手
maskrun import .env --dry-run
maskrun import .env

# 2. 检查清单里要求的每个名字是否真的都在
maskrun status

# 3. 在磁盘上没有 .env 的情况下运行你的应用
maskrun run -- npm run dev

# 到这一步，才删掉 .env
```

`import` 会在你的代码旁边写一份 `.maskrun` 清单：

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**这个文件存的是名字，不是值。请提交它。** 它才是真正管用的 `.env.example`：
`maskrun status` 会准确告诉新同事他缺哪些密钥。

## 工作原理

三种机制，对应三种不同的需求。

| 需求 | 机制 | 代理看到的内容 |
|---|---|---|
| 需要 `DATABASE_URL` 的开发服务器、数据库迁移或 CLI | `maskrun run` 注入子进程 | 被遮蔽的输出（`<masked:VAR>`） |
| 值本身 —— 轮换密钥、粘贴到某个控制台 | 你自己，在你自己的终端里 | 什么都看不到；护栏会拒绝 |
| 知道**有哪些**密钥存在 | `maskrun list`、`maskrun status` | 只有名字，绝不含值 |

### 是注入，不是读取

值存在钥匙串里。`maskrun run` 解析清单，把值交给恰好一个子进程，不在磁盘上留下
任何东西。只要缺了任何一个密钥，它宁可拒绝启动，也不会让你的应用带着半个环境跑
起来。

### 输出遮蔽

`run` 和 `exec` 会把子进程的 stdout 和 stderr 送进一个过滤器，把密钥的值替换成
`<masked:VAR>`。

- 值通过**过滤器自己的环境变量**送达，绝不走 argv —— argv 可以通过
  `/proc/<pid>/cmdline` 读到。
- 除原始值外，也遮蔽它的 base64、URL 编码和反斜杠转义写法。
- 能抓住**被拆到两次写入里**的值，所以横跨刷新边界的密钥不会漏过去。
- 完整的行会立刻放行，这样你盯着看的时候，开发服务器的输出不会卡在缓冲里。
- 拒绝遮蔽短于 6 个字符的值，并且会明说原因：遮蔽 `5432` 会把输出里每一个无关的
  数字都弄坏。
- 退出码、stdin 和 Ctrl-C 原样传给子进程。

在 AI 代理会话中，以及 stdout 不是终端的任何时候，默认开启；在你自己的交互式终端
里默认关闭，以保留颜色。`--raw` 强制关闭，`--mask` 强制开启。

### 代理护栏

```bash
maskrun install-guard        # 为 Claude Code 注册一个 PreToolUse 钩子
```

钩子的全部意义在于：**执行它的是 harness，不是模型**。写进提示词里的规则由模型自己
执行，所以模型能把自己说服过去。这个说服不了。

它会拒绝那些会把值送进对话记录的命令：

- `maskrun get`、`maskrun import`
- `secret-tool lookup/search`、`security find-generic-password`
- `--raw`、`MASKRUN_MASK=0`、`MASKRUN_ALLOW_READ=1`（关闭遮蔽）
- `/proc/<pid>/environ`
- 裸的 `env` / `printenv`、`Get-ChildItem Env:`
- 读取 `.env` / `.envrc`，无论走 Bash 还是代理自己的文件工具

同时它不打扰正常工作：`maskrun run`、`env VAR=x cmd`、`cat .env.example`、
`cat .maskrun`、`printenv PATH`、`ls -la .env`、`rm .env`。

`maskrun install-guard` 会合并进你现有的 `settings.json`，先备份，是幂等的，遇到
不是合法 JSON 的文件会拒绝改动，`--remove` 可以撤销。其他 harness：把
`maskrun hook` 作为工具调用前的钩子来运行，它从 stdin 以 JSON 形式接收工具调用 ——
参见 [integrations/claude-code](integrations/claude-code/)。

CLI 自身也执行同样的拒绝规则，所以即便代理跑在不支持钩子的 harness 里，它依然
无法执行 `maskrun get`。

### 引导没有钩子机制的代理

```bash
maskrun install-rules        # 向 AGENTS.md/CLAUDE.md/.cursor 规则写入一小段内容
```

`install-guard` 只在 harness 替你运行 PreToolUse 钩子的地方有效。在别处，没有任何
东西在强制执行任何规则 —— 所以 `install-rules` 会把一小段带标记的内容，写进项目
中已经存在的 `AGENTS.md`、`CLAUDE.md` 或 `.cursor/rules/`（若一个都没有则创建
`AGENTS.md`），告诉代理在这里该怎么用 maskrun。**这是引导，不是强制**：模型可以
忽略它，就像它可以忽略任何其他指令一样。它是给 `install-guard` 够不着的 harness
准备的退路，不是它的替代品。

这段内容由 `<!-- maskrun:start -->`/`<!-- maskrun:end -->` 标记界定，写入前先备份
文件，且是幂等的（第二次运行会就地更新而不是重复添加），`--remove` 可以把它取出来
而不碰文件的其余部分。如果存在 `.maskrun` 清单，这段内容会列出项目实际需要的变量
名；`--file <path>` 直接指定单个文件，跳过查找。

<a id="interactive-view"></a>

### 交互式视图

```bash
maskrun            # 在你自己的终端里，不带参数
```

在命令行上没有其他内容的真实终端中，会打开一个方向键操作的视图：左边是平台，右边
是该平台下的密钥，以及高亮项的详情（平台、备注、值）。

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

值一直处于遮蔽状态（`••••••••••••••••`），直到你按下 `v`；而且值只在那一刻才从
钥匙串里读取 —— 在列表里移动完全不会碰它。切换到另一个密钥会自动重新遮蔽。`e`
就地编辑平台、备注和值（留空则保留当前值）；`d` 要求按名字确认
（`delete 'name'? [y/N]`），只有字面的 `y`/`Y` 才会继续 —— 包括回车在内的其他任何
按键都是取消。`y` 把值复制到剪贴板。

它运行在[备用屏幕缓冲区](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer)
上：它绘制的任何内容 —— 包括已显示出来的值 —— 都不会留在终端的回滚缓冲里，这与
今天 `maskrun get` 的输出不同。抛开下面所有其他讨论，这本身就是一项实打实的改进。

和其他每一条会接触到值的路径一样，它**绝不会在 AI 代理会话中打开** —— 无论输出
是否被管道接走，一旦检测到是代理会话，得到的永远是裸 `maskrun` 在非交互模式下打印
的那份只含名字的摘要。在小于 60x15 的终端里它也会拒绝打开，并在退回到那份摘要之前
告诉你原因。

#### 复制到剪贴板

`y` 把当前密钥复制到剪贴板 —— 因为显示出一个你随后哪儿也粘贴不了的值，只是把问题
往后推：你要么重新敲一遍，要么用鼠标选中，两者都更糟。内置三重限制：

- **45 秒后自动清除。** 一旦复制了内容，TUI 会显示实时倒计时；`c` 可以立即清除而
  不必等待，带着尚未清除的复制内容退出 TUI 同样会清除。清除时会恢复复制之前剪贴板
  里的内容，若原本为空则清空。
- **在所有平台上都会发出「不要记录」的提示**，通过 arboard 的
  `exclude_from_history`：Linux 上是 KDE 的 `x-kde-passwordManagerHint` MIME
  类型，macOS 上是社区约定的 `org.nspasteboard.ConcealedType`，Windows 上是原生的
  `CanIncludeInClipboardHistory` 剪贴板格式。这里已针对真实的 Klipper 验证过：普通
  复制会出现在它的历史里，带标记的不会，而且 Klipper 甚至不会把带标记的那份报告为
  **当前**剪贴板内容。每一种都只是某个具体工具选择遵守的提示，而不是 maskrun 能够
  强制的事 —— 参见下文。
- 它**绝不会在代理会话中打开**，和 TUI 其余部分是同一道门槛。

**这解决不了的问题：** 任何不去查看该提示的剪贴板历史工具（GNOME 的、大多数非 KDE
的 Wayland 环境，以及 —— 在开发本功能时直接观察到的 —— 当 KDE 自家的 Klipper 没有
监听你的合成器所用的那条剪贴板传输通道时，连它也算 ——）依然会把值永久记录下来；
而一旦那份副本进了历史文件，45 秒的自动清除对它毫无作用。请把复制到剪贴板，与下文
的 `/proc/<pid>/environ` 以及人类手动粘贴未遮蔽的值放在同一类看待：这是一个真实的
限制，不是一个已解决的问题。

## 这不是什么

**maskrun 不是安全边界。** 它是针对意外的加固，不应该被卖给你 —— 或者被你卖给
别人 —— 说成更多的东西。

- **钥匙串解决的是存储，不是访问。** 以你的身份运行的每一个进程都可以调用
  `secret-tool lookup` 或 `security find-generic-password`，包括你的代理。护栏抬高
  了不小心这么做的代价；它并不能让这件事变得不可能。对你自己也一样：如果有人手动
  把未遮蔽的值粘进对话记录，那一次按键之后的任何工具都拦不住它。
- **护栏读不懂的代码。** heredoc 的内容和脚本文件被当作数据而非命令处理 —— 这是
  刻意为之，因为解析它们会产生误报。所以一个自己从自身源码里读取 `.env` 的脚本会
  通过。模式匹配永远抓不住任意代码。
- **进程环境。** 在 `maskrun run` 运行期间，它子进程的环境可以通过
  `/proc/<pid>/environ` 读到。护栏直接封堵了这条路径，但环境注入从设计上就是这个
  形状。
- **自动清除并不能撤销复制到剪贴板（交互式视图里的 `y`）。** 45 秒超时清空的是
  **剪贴板**；对于在超时触发之前就已经把值永久记录下来的历史工具，它无能为力。
  maskrun 会给这次复制打标记，让 KDE 的 Klipper 跳过它 —— 这是真实且已验证的；但
  并非每个保存历史的工具都遵守该标记，而且据我们所知，GNOME 的剪贴板历史、大多数
  非 KDE 的 Wayland 环境以及 Windows 剪贴板历史都不遵守。它和上面的
  `/proc/<pid>/environ` 以及下面「人类手动粘贴未遮蔽的值」摆在同一格架子上：一个
  明说出来的限制，而不是被悄悄「解决」掉的问题。
- **写入期间的 `ps` —— 已封堵。** 在 macOS 上，值过去是作为命令行参数传给
  `security` 的，会短暂出现在 `ps` 的进程列表里。这条路径已经没有了：maskrun 现在
  直接调用 Security.framework 的 generic-password API，所以值根本不会变成操作系统
  必须暴露给任何人的 argv。
- **是按管道分段匹配，不是 shell 解析器。** 护栏会单独评估管道中的每一段，正因
  如此，`sed 's/maskrun get/x/' notes.md` 才能通过 —— 这段文本从未调用
  `maskrun get`，只是提到了它。同样的作用域也意味着护栏不会顺着间接调用追踪一条
  命令：`echo 'maskrun get x' | sh` 被读作一个 `echo`，而不是 `sh` 最终执行的那条
  命令。

如果你想要一条真正的边界，代理的 shell 必须运行在根本够不到钥匙串的地方 ——
Linux 上没有 D-Bus 会话套接字、一个独立的用户账号，或者一个容器里。那时 maskrun
从你的终端可用，而从代理的终端不可用。影响范围在服务商那一侧管理效果更好：每个
工具一把独立的密钥、消费上限、轮换，以及在有条件的地方使用短期凭据。

## 后端

| 平台 | 后端名称 | 存储 |
|---|---|---|
| Linux | `secret-service` | libsecret 的 Secret Service，直接走 D-Bus（`dbus-secret-service` crate）—— gnome-keyring、KWallet、KeePassXC |
| macOS | `keychain` | 登录钥匙串，经由 Security.framework 的 generic-password API（`security-framework` crate） |
| Windows | `dpapi` | Windows 凭据管理器（`windows` crate，`Win32_Security_Credentials`）—— 这个名字是为了覆盖变量的兼容性而沿用旧的 DPAPI 文件后端，底下的存储已经不再是 DPAPI 文件 |

早期版本在每次操作时都会在 Linux 上调用 `secret-tool`、在 macOS 上调用 `security`
作为子进程，并在 Windows 上手写 DPAPI 文件存储。现在三者的 `put`/`get`/`delete`
都改为通过一个绑定到平台 API 的库，而不是子进程或手写的加密代码 —— Linux 上不再
需要安装 `secret-tool`，macOS 上密钥的值也不再成为命令行参数。（macOS 上的 `list`
仍然会调用 `security dump-keychain` 来枚举名字 —— 这个调用不以任何密钥的值作为
参数，所以没有暴露任何新东西；`security-framework` 里没有可以替代它的、按服务限定
范围的枚举调用。）

Linux 的存储保留了 `secret-tool lookup service maskrun name <name>` 所期望的
schema（`service` + `name` 属性），所以如果你需要手工核对，maskrun 存下的密钥仍然
可以用标准 CLI 读出来 —— 只是 maskrun 自身不再依赖那个 CLI 是否已安装。

### 平台标签和备注存在哪里

| 平台 | 存放位置 |
|---|---|
| Linux | 在既有的 `service`/`name` 之外，增加两个 Secret Service 属性 `platform` 和 `note` —— `secret-tool lookup` 只按你给出的属性匹配，所以这两个属性只是顺带存在，不影响任何已经在读 `service`+`name` 的东西。 |
| macOS | generic password 的 `kSecAttrDescription`（平台）和 `kSecAttrComment`（备注）字段，通过 `security-framework` 的属性搜索 API 和仅更新属性的 API 读写 —— 重新打标签完全不会触碰已存储的值本身。 |
| Windows | 两者都编码进凭据管理器提供的那唯一一个 `CREDENTIALW.Comment` 字段（`platform=<p><US>note=<n>`，`<US>` = U+001F）：这里没有单独的属性存储，而且由于 `CredWriteW` 没有部分更新的调用，重新打标签必须重写整条记录（包括值）。 |

用 `MASKRUN_BACKEND=secret-service|keychain|dpapi` 覆盖自动检测。

### 实际验证到什么程度

三个后端都在 CI 中针对真实钥匙串运行过：Linux 上的 Secret Service、macOS 上的
Keychain、Windows 上的凭据管理器。钥匙串测试会真正往返一个真实的密钥，而不是
mock 掉后端；另有一个完全没有安装钥匙串的任务，用来证明护栏依然会作出应答。

平台标签 / 备注这个功能也是同样的情况：在 Linux 上针对真实的 Secret Service 运行过
（包括 `secret-tool` 与属性 schema 的兼容性检查），macOS 和 Windows 部分则与其余
平台代码一样做了跨目标类型检查，但真正在真实的 Keychain 或凭据管理器上运行，要等
那些 CI 任务跑起来之后才算数。

第一次真正启动起来的 CI 运行，在平台代码里找出了三个货真价实的 bug，全都位于
Linux 构建从不做类型检查的路径上，因为它们受 `cfg` 保护：两个类型写错的 Win32
参数；一次 `CredEnumerateW` 调用同时传入了过滤条件和「全部凭据」标志（两者不能
同用）；以及把 `ERROR_NOT_FOUND` 与原始 Win32 错误码而非实际返回的 `HRESULT` 作
比较 —— 这使得「密钥不存在」和「存储为空」两种情况都以原始错误的形式冒出来。现在
lint 任务会从 Linux 交叉检查 Windows 和 macOS 目标，让这类错误再也到不了平台
runner。

**没有**覆盖到的是：锁处理相关的测试需要显式开启（`MASKRUN_LOCK_TESTS=1`），因为
在真实桌面上启动一个一次性的 `gnome-keyring-daemon` 会弹窗要求用户创建钥匙串，而且
它的存活时间比测试还长。

## 命令

```
maskrun put <name>                    store a secret (prompts; not echoed)
maskrun put                           fully interactive: asks name, platform, note, value
maskrun put <name> --stdin            store from stdin (for scripts)
maskrun put <name> --for X --note Y   tag it with a platform and a note while storing
maskrun label <name> --for X          tag (or retag) an existing secret's platform
maskrun label <name> --for ""         clear its platform
maskrun get <name>                    print it (refused in an agent session)
maskrun list                          grouped by platform, with notes, never values
maskrun list --plain                  flat, sorted names only, one per line (for scripts)
maskrun rm <name>                     delete (no name: pick from a numbered list)
maskrun status                        check the manifest against the keyring
maskrun run [--raw|--mask] -- CMD     run with the manifest injected
maskrun exec VAR=name -- CMD          run with explicit pairs, no manifest
maskrun -- CMD                        shorthand for run
maskrun import <.env> [--dry-run]     move a .env into the keyring
maskrun completions <fish|bash|zsh>   print a shell completion script
maskrun install-guard [--remove]      register the agent guard
maskrun hook                          the guard itself (reads JSON on stdin)
maskrun install-rules [--remove]      tell agents how to use maskrun here (guidance, not enforcement)
```

`import` 会跳过 `VITE_`、`NEXT_PUBLIC_`、`PUBLIC_`、`REACT_APP_`、`NUXT_PUBLIC_`、
`EXPO_PUBLIC_` 和 `GATSBY_` 开头的变量：它们会被编译进你的客户端产物并分发给每一位
访客，因此属于配置而不是密钥，搬过来没有任何收益。`--all` 可以覆盖这一行为。

### 平台标签与备注

密钥一旦超过几个，光靠一个扁平的名字就记不住每个是干什么用的了 —— 尤其是当一个
平台有不止一把密钥，而它们之间的差别在于作用范围而不是名字的时候。`--for` 给密钥
打上平台／服务标签；`--note` 添加一段简短的自由文本说明：

```
$ maskrun put gh-release-token --for github --note "fine-grained, contents+plan"
stored: gh-release-token

$ maskrun label vault-token-github --for github --note "fine-grained, repo+plan"
labelled: vault-token-github

$ maskrun list
github
  gh-release-token       fine-grained, contents+plan
  vault-token-github     fine-grained, repo+plan
(no platform)
  pos-terminal-electron-api-url
```

两个选项都是可选的，适用于目前两者都没有的密钥。`label` 用来事后给已有的密钥重新
打标签；在这两个命令中，传入 `--for ""`（或 `--note ""`）会清空该字段，而完全不写
某个选项则保持现有值不变 —— 用 `put` 覆盖某个密钥的值，绝不会悄悄丢掉它的标签。
**平台和备注不是密钥**：它们不加遮蔽地存储，在 AI 代理会话中可见，并且会被 `list`
和 `status` 显示出来。不要把密钥的值放进 `--note`；它上限 200 个字符，且不能包含
换行。

不带参数的 `maskrun` 会在真实终端里打开[交互式视图](#interactive-view)；若被管道
接走（`maskrun | cat`）、处于非交互环境，或身处代理会话中，它会改为打印一份简短
概览：清单状态，以及上面那份按平台分组的密钥列表 —— 这样你就不必把整套命令都记在
脑子里。

### Shell 补全

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # 然后在 compinit 之前加上 `fpath+=~/.zfunc`
```

除了选项和子命令之外，`maskrun get`/`rm`/`label` 还会补全真实的密钥名字 —— fish
开箱即可做到（正是通过这个选项存在的理由：`maskrun list --plain`）；bash 和 zsh
只有静态补全。

## 环境变量

| 变量 | 作用 |
|---|---|
| `MASKRUN_MASK` | `1` 始终遮蔽，`0` 从不遮蔽 |
| `MASKRUN_AGENT` | `1` 视作代理会话，`0` 视作人类 |
| `MASKRUN_BACKEND` | 强制指定后端 |
| `MASKRUN_ALLOW_READ` | `1` 在代理会话中重新启用 `get`/`import` |

代理会话通过 `CLAUDECODE`、`CLAUDE_CODE_ENTRYPOINT`、`AI_AGENT`、`AIDER_CHAT`、
`CURSOR_AGENT`、`OPENAI_CODEX`、`GEMINI_CLI` 和 `REPLIT_AGENT` 来识别。其他情况
请在 harness 中设置 `MASKRUN_AGENT=1`。

## 平台支持

- **Linux** —— glibc 2.35 或更高（发布版二进制文件在 Ubuntu 22.04 上构建）。已在
  CI 中针对运行中的 Secret Service 测试。
- **macOS** —— Intel 和 Apple Silicon。已在 CI 中针对真实的 Keychain 测试。
- **Windows** —— x86_64。已在 CI 中针对真实的凭据管理器测试。

## 相关前作

[`envchain`](https://github.com/sorah/envchain) 多年前就把密钥放进钥匙串并注入
环境变量，是本工具 `run` 那一半的直系祖先。[`direnv`](https://direnv.net/) 管理
按目录划分的环境，[`sops`](https://github.com/getsops/sops) 和
[`dotenvx`](https://github.com/dotenvx/dotenvx) 在仓库里对静态密钥加密，
[`aws-vault`](https://github.com/99designs/aws-vault) 则为某一家服务商完成钥匙串
那一套流程。

maskrun 新增的是面向代理的那一半：遮蔽子进程的输出，以及由 harness 强制执行的、
针对那些会泄露值的命令的护栏。如果你不和 AI 代理一起工作，`envchain` 也许就够用了。

## 开发

```bash
cargo test -- --test-threads=1   # 单线程：各后端共享真实的钥匙串状态
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # 同一轮测试
make lint                        # fmt --check + clippy
```

当没有任何可用后端时，钥匙串测试会自行跳过，这样在一个干净的容器里护栏测试依然
能跑。

测试套件里的密钥值是随机生成的，并且从不打印 —— 断言检查的是「不存在」或「存在」，
绝不会去和某个被记录下来的值做相等比较。

欢迎贡献。新增一个后端意味着实现 `Backend` trait 的
`put`/`get`/`delete`/`list`；新增一个 harness 意味着在 `integrations/` 下添加一个
条目。

## 许可证

Copyright (C) 2026 Furkan Akyol.

maskrun 是自由软件：你可以依照自由软件基金会发布的 GNU 通用公共许可证第 3 版，或
（由你选择的）任何更新的版本的条款，重新分发和修改它。它不附带任何担保。完整条款
参见 [LICENSE](LICENSE)。

一个实际后果是：如果你分发修改过的 maskrun —— 无论是以源码、以二进制文件，还是
包含在某个产品之中 —— 你都必须在同一许可证下，把你修改后的源码提供给接收方。
