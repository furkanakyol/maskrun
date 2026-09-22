[English](README.md) · [简体中文](README.zh-CN.md) · [Español](README.es.md) · [Português (BR)](README.pt-BR.md) · [Русский](README.ru.md) · **日本語** · [Français](README.fr.md) · [Deutsch](README.de.md) · [Türkçe](README.tr.md)

> これは英語版 README の翻訳です。内容に食い違いがある場合は
> [英語版](README.md) が正となります。

# maskrun

OS のキーリングにある秘密情報を使ってコマンドを実行し、その値を AI コーディング
エージェントのコンテキストの外に保つツール。

```bash
maskrun put myapp-database-url        # キーリングに保存、ディスクには一切書かない
maskrun run -- npm run dev            # 子プロセスに注入し、出力ではマスクする
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

単一バイナリ、ランタイム依存なし、デーモンなし。Linux、macOS、Windows 対応。

---

## 問題

秘密情報を `.env` ファイルから OS のキーリングへ移すのは簡単なほうの半分です。
難しいほうは、AI エージェントがあなたのシェルを操作し始めた瞬間に現れます。

秘密情報を子プロセスへ注入すれば、エージェントが *保管庫を読む* ことは防げます。
しかし値が *プロセスから出てくる* ことには何の効果もありません。

- 開発サーバーが起動時に接続文字列を表示する
- `curl -v` が `Authorization` ヘッダーをそのまま出す
- スタックトレースが DSN を含んでいる
- `psql` が接続に失敗した URL を引用して表示する

どれか一つでも起きれば、秘密情報はトランスクリプトに入り、会話の一部となり、
スクロールバックの一部となり、そのトランスクリプトが保存されるあらゆる場所の
一部になります。

maskrun はこの経路と、その隣にある経路を塞ぎます。

## インストール

**Linux / macOS** — ビルド済みバイナリをダウンロードし、チェックサムを検証します。
コンパイラは不要です。

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**Rust ツールチェーンから:**

```bash
cargo install maskrun
```

**ソースから:**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# バイナリは target/release/maskrun
```

## クイックスタート

```bash
# 1. 既存の .env をキーリングへ移す — 飛ぶ前にまず見る
maskrun import .env --dry-run
maskrun import .env

# 2. マニフェストが要求する名前が実際にすべて揃っているか確認する
maskrun status

# 3. ディスク上に .env がない状態でアプリを起動する
maskrun run -- npm run dev

# .env を削除するのはここまで来てから
```

`import` はコードの隣に `.maskrun` マニフェストを書き出します。

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**このファイルが持つのは名前であって値ではありません。コミットしてください。**
これは実際に役に立つ `.env.example` です。新しく加わったチームメンバーに、
どの秘密情報が足りないかを `maskrun status` が正確に伝えます。

## 仕組み

3 つの異なるニーズに対する、3 つの仕組み。

| ニーズ | 仕組み | エージェントに見えるもの |
|---|---|---|
| `DATABASE_URL` を必要とする開発サーバー、マイグレーション、CLI | `maskrun run` が子プロセスへ注入する | マスクされた出力 (`<masked:VAR>`) |
| 値そのもの — 鍵のローテーション、ダッシュボードへの貼り付け | あなた自身が、自分の端末で | 何も見えない。ガードが拒否する |
| *どの* 秘密情報が存在するかを知ること | `maskrun list`、`maskrun status` | 名前だけ。値は決して見せない |

### 読み取りではなく注入

値はキーリングの中にあります。`maskrun run` はマニフェストを解決し、値をちょうど
1 つの子プロセスに渡し、ディスクには何も保存しません。秘密情報が 1 つでも欠けて
いれば、環境が半分そろっていない状態でアプリを起動するのではなく、起動そのものを
拒否します。

### 出力のマスク

`run` と `exec` は、子プロセスの stdout と stderr を、秘密情報の値を
`<masked:VAR>` に置き換えるフィルターに通します。

- 値は **フィルター自身の環境変数** 経由で渡され、argv 経由では決して渡されません
  — argv は `/proc/<pid>/cmdline` から読めるためです。
- 生の値に加えて、その base64、URL エンコード、バックスラッシュエスケープの各表記も
  マスクします。
- **2 回の書き込みにまたがって分割された** 値も捕まえるので、フラッシュ境界を
  またいだ秘密情報がすり抜けることはありません。
- 完成した行は直ちに通すので、開発サーバーの出力を見ている間にバッファで
  止まることはありません。
- 6 文字未満の値のマスクは拒否し、その旨を伝えます。`5432` をマスクすれば、出力中の
  無関係な数字がすべて壊れてしまうからです。
- 終了コード、stdin、Ctrl-C はそのまま子プロセスに届きます。

AI エージェントのセッション中、および stdout が端末でない場合は既定で有効。
自分の対話端末では色が保たれるよう無効になります。`--raw` は強制的に無効、
`--mask` は強制的に有効にします。

### エージェント用のガード

```bash
maskrun install-guard        # Claude Code 用の PreToolUse フックを登録する
```

フックの要点は、**強制するのがモデルではなくハーネスである** ことです。プロンプトに
書かれた規則を強制するのはモデル自身なので、モデルは自分でその規則から抜け出す
理屈をつけられます。こちらは言葉で抜け出せません。

値をトランスクリプトに乗せてしまうコマンドを拒否します。

- `maskrun get`、`maskrun import`
- `secret-tool lookup/search`、`security find-generic-password`
- `--raw`、`MASKRUN_MASK=0`、`MASKRUN_ALLOW_READ=1`（マスクを無効にするもの）
- `/proc/<pid>/environ`
- 引数のない `env` / `printenv`、`Get-ChildItem Env:`
- `.env` / `.envrc` の読み取り — Bash 経由でも、エージェントのファイルツール
  経由でも

一方、通常の作業には手を出しません。`maskrun run`、`env VAR=x cmd`、
`cat .env.example`、`cat .maskrun`、`printenv PATH`、`ls -la .env`、`rm .env`。

`maskrun install-guard` は既存の `settings.json` にマージし、先にバックアップを
取り、冪等であり、正しい JSON でないファイルには手を触れることを拒否します。
`--remove` で元に戻せます。他のハーネスの場合は、ツール呼び出しを JSON として
stdin で受け取るプレツールフックとして `maskrun hook` を実行してください —
[integrations/claude-code](integrations/claude-code/) を参照。

CLI 自身も同じ拒否を実装しているので、フックに対応していないハーネスで動いている
エージェントであっても `maskrun get` はできません。

### フック機構を持たないエージェントを導く

```bash
maskrun install-rules        # AGENTS.md/CLAUDE.md/.cursor のルールに短いブロックを書き込む
```

`install-guard` が効くのは、ハーネスがあなたの代わりに PreToolUse フックを実行して
くれる場所だけです。それ以外の場所では何も強制されません。そこで
`install-rules` は、プロジェクトに既に存在する `AGENTS.md`、`CLAUDE.md`、
`.cursor/rules/` のいずれかに（どれもなければ `AGENTS.md` を作成して）、目印付きの
短いブロックを書き込み、ここでの maskrun の使い方をエージェントに伝えます。
**これは指示であって強制ではありません**。モデルは他のどんな指示とも同じように、
これを無視できます。`install-guard` が届かないハーネスのための代替手段であり、
その代わりになるものではありません。

このブロックは `<!-- maskrun:start -->`/`<!-- maskrun:end -->` のマーカーで囲まれ、
先にファイルのバックアップを取り、冪等です（2 回目の実行は重複させずその場で更新
します）。`--remove` はファイルの他の部分に触れずにブロックだけを取り除きます。
`.maskrun` マニフェストがあれば、ブロックはプロジェクトが実際に期待する変数名を
列挙します。`--file <path>` は探索を省いて特定のファイルを直接指定します。

<a id="interactive-view"></a>

### 対話ビュー

```bash
maskrun            # 自分の端末で、引数なしで
```

コマンドラインに他に何もない本物の端末では、矢印キーで操作するビューが開きます。
左にプラットフォーム、右にそのプラットフォームの秘密情報と、選択中の項目の詳細
（プラットフォーム、メモ、値）が並びます。

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

値は `v` を押すまでマスクされたまま（`••••••••••••••••`）で、値がキーリングから
読まれるのはその瞬間だけです。リストを移動するだけでは値に一切触れません。別の
秘密情報へ移動すると自動的に再びマスクされます。`e` はプラットフォーム、メモ、値を
その場で編集します（空のままにすれば現在の値を保持）。`d` は名前による確認を求め
（`delete 'name'? [y/N]`）、文字どおりの `y`/`Y` だけが先へ進みます。Enter を含む
他のキーはすべてキャンセルです。`y` は値をクリップボードへコピーします。

このビューは [代替スクリーンバッファ](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer)
上で動きます。描画される内容は — 表示された値も含めて — 端末のスクロールバックに
一切残りません。現在の `maskrun get` の出力とは対照的です。これは以下のどの話とも
独立した、実質的な改善です。

値に触れる他のあらゆる経路と同じく、**AI エージェントのセッションでは決して
開きません**。出力がパイプされているかどうかに関係なく、エージェントセッションと
検出された場合は常に、引数なしの `maskrun` が非対話的に表示するのと同じ、名前だけの
概要が返ります。また 60x15 より小さい端末では起動を断り、その概要に切り替える前に
理由を伝えます。

#### クリップボードへのコピー

`y` は現在の秘密情報をクリップボードへコピーします。どこにも貼り付けられない値を
表示するだけでは問題を先送りするだけで、結局は打ち直すかマウスで選択することになり、
どちらもより悪いからです。組み込みの制限は 3 つあります。

- **45 秒後に自動で消去されます。** 何かをコピーすると TUI がカウントダウンを表示
  します。`c` は待たずに即座に消去し、何かをコピーしたまま TUI を終了した場合も
  消去されます。消去時には、コピー前にクリップボードにあった内容を復元し、何も
  なければ空にします。
- **すべてのプラットフォームで「記録しないで」というヒントを送ります。** arboard の
  `exclude_from_history` 経由で、Linux では KDE の `x-kde-passwordManagerHint`
  MIME タイプ、macOS ではコミュニティ慣習の `org.nspasteboard.ConcealedType`、
  Windows ではネイティブの `CanIncludeInClipboardHistory` クリップボード形式を
  使います。ここでは実際の Klipper で検証済みです。通常のコピーはその履歴に現れ、
  タグ付きのコピーは現れず、Klipper はタグ付きのものを *現在の* クリップボード内容
  としてすら報告しません。いずれも特定のツールが従うことを選ぶヒントであって、
  maskrun が強制できるものではありません — 下記を参照。
- **エージェントセッションでは決して開きません。** TUI の他の部分と同じ関門です。

**これで解決しないこと:** そのヒントを見ていないクリップボード履歴ツール
（GNOME のもの、KDE 以外の Wayland 環境のほとんど、そして — これを作りながら直接
観測したのですが — あなたのコンポジタが使っているクリップボードの伝送経路を
監視していないときの KDE 自身の Klipper さえも）は、依然として値を永続的に記録
します。45 秒の自動消去は、いったん履歴ファイルに入ったそのコピーには何もしません。
クリップボードへのコピーは、下記の `/proc/<pid>/environ` や、マスクされていない値を
人間が手で貼り付ける行為と同じ扱いをしてください。解決済みの問題ではなく、現実の
限界です。

## これは何ではないか

**maskrun はセキュリティ境界ではありません。** 事故に対する堅牢化であって、それ以上の
ものとしてあなたに売られるべきでも、あなたが売るべきでもありません。

- **キーリングが解決するのは保管であって、アクセスではありません。** あなたの
  ユーザーとして動くすべてのプロセスは `secret-tool lookup` や
  `security find-generic-password` を呼べます。あなたのエージェントも含めてです。
  ガードは、それをうっかりやってしまう代償を高くしますが、不可能にはしません。
  あなた自身についても同じです。人間がマスクされていない値を手でトランスクリプトに
  貼り付けたなら、そのキー入力より下流のどんなツールもそれを捕まえられません。
- **ガードが読めないコード。** ヒアドキュメントの中身やスクリプトファイルは、
  コマンドではなくデータとして扱われます。これは意図的で、それらを解析すると誤検知が
  出たからです。したがって、自分のソースの中から `.env` を自分で読むスクリプトは
  通ってしまいます。パターンマッチが任意のコードを捕まえることは決してありません。
- **プロセスの環境変数。** `maskrun run` の実行中、その子プロセスの環境は
  `/proc/<pid>/environ` から読めます。ガードはこの経路を直接ブロックしますが、
  環境変数への注入は設計上そういう形をしています。
- **クリップボードへのコピー（対話ビューの `y`）は自動消去では取り消せません。**
  45 秒のタイムアウトが空にするのは *クリップボード* です。タイムアウトが発動する前に
  値を永続的に記録してしまった履歴ツールには何もしません。maskrun は KDE の Klipper が
  読み飛ばすようコピーにタグを付けます — これは実際に検証済みですが、履歴を保持する
  すべてのツールがそのタグに従うわけではなく、GNOME のクリップボード履歴、KDE 以外の
  Wayland 環境のほとんど、Windows のクリップボード履歴が従うという情報はありません。
  上記の `/proc/<pid>/environ` や、下記の「マスクされていない値を人間が手で貼り付ける」
  と同じ棚に置かれるもの。黙って解決したふりをせず、はっきり述べた限界です。
- **書き込み中の `ps` — こちらは塞ぎました。** macOS では以前、値が CLI 引数として
  `security` に渡っており、`ps` 経由でプロセス一覧に一瞬見えていました。その経路は
  もうありません。maskrun は現在 Security.framework の generic-password API を直接
  呼ぶので、値が OS の誰かに晒さざるを得ない argv になることは一切ありません。
- **シェルのパーサーではなく、パイプラインのセグメント単位の照合。** ガードは
  パイプラインの各セグメントを個別に評価します。だからこそ
  `sed 's/maskrun get/x/' notes.md` は通ります。そのテキストは `maskrun get` を
  呼び出しておらず、言及しているだけだからです。同じスコープであるということは、
  ガードが間接参照を通じてコマンドを追跡しないということでもあります。
  `echo 'maskrun get x' | sh` は `echo` として読まれ、`sh` が最終的に実行する
  コマンドとしては読まれません。

本物の境界が欲しいなら、エージェントのシェルはキーリングにまったく到達できない場所で
動かす必要があります。Linux なら D-Bus のセッションソケットなし、別のユーザー
アカウント、あるいはコンテナです。そうすれば maskrun はあなたの端末からは動き、
エージェントの端末からは動きません。被害範囲はプロバイダー側で管理するほうが
うまくいきます。ツールごとに別の鍵、利用上限、ローテーション、そして提供されている
ところでは短命の認証情報を。

## バックエンド

| プラットフォーム | バックエンド名 | ストレージ |
|---|---|---|
| Linux | `secret-service` | libsecret の Secret Service を D-Bus 経由で直接（`dbus-secret-service` クレート）— gnome-keyring、KWallet、KeePassXC |
| macOS | `keychain` | ログインキーチェーン。Security.framework の generic-password API 経由（`security-framework` クレート） |
| Windows | `dpapi` | Windows 資格情報マネージャー（`windows` クレート、`Win32_Security_Credentials`）— 名前は上書き互換性のため旧 DPAPI ファイルバックエンドから引き継いだもので、その下のストレージはもう DPAPI ファイルではありません |

以前のバージョンは、操作のたびに Linux では `secret-tool`、macOS では `security` を
サブプロセスとして呼び出し、Windows では DPAPI のファイル保管を自前で実装していました。
現在は 3 つとも、`put`/`get`/`delete` についてサブプロセスや手書きの暗号処理ではなく、
プラットフォーム API へのライブラリバインディングを経由します。Linux で
`secret-tool` をインストールする必要はなく、macOS では秘密情報の値が CLI 引数に
なることもありません。（macOS の `list` は名前を列挙するために今も
`security dump-keychain` を呼びます。この呼び出しは秘密情報の値を引数に取らないので、
新たに晒されるものはありません。`security-framework` には、これを置き換えられる
サービス単位の列挙呼び出しがないためです。）

Linux のストレージは `secret-tool lookup service maskrun name <name>` が期待する
スキーマ（`service` + `name` 属性）を保っています。したがって maskrun が保存した
秘密情報は、手で確認したいときには標準の CLI で今も読めます。maskrun 自身が、その
CLI のインストールに依存しなくなっただけです。

### プラットフォームラベルとメモの保存場所

| プラットフォーム | 場所 |
|---|---|
| Linux | 既存の `service`/`name` と並ぶ 2 つの追加 Secret Service 属性 `platform` と `note`。`secret-tool lookup` は与えた属性だけで照合するので、これらは相乗りするだけで、既に `service`+`name` を読んでいるものには何の影響もありません。 |
| macOS | generic password の `kSecAttrDescription`（プラットフォーム）と `kSecAttrComment`（メモ）フィールド。`security-framework` の属性検索 API と属性のみ更新 API で読み書きします。ラベルを付け替えても、保存された値そのものには一切触れません。 |
| Windows | 資格情報マネージャーが提供する唯一の `CREDENTIALW.Comment` フィールドに両方をエンコードします（`platform=<p><US>note=<n>`、`<US>` = U+001F）。ここには別個の属性ストアがなく、`CredWriteW` に部分更新の呼び出しがないため、ラベルの付け替えはエントリ全体（値を含む）を書き直す必要があります。 |

検出を上書きするには `MASKRUN_BACKEND=secret-service|keychain|dpapi`。

### 実際に検証済みのこと

3 つのバックエンドはすべて、CI で本物のキーリングに対して実行されています。Linux では
Secret Service、macOS では Keychain、Windows では資格情報マネージャーです。キーリングの
テストはバックエンドをモックせず、実際の秘密情報を往復させます。また、キーリングが
まったくインストールされていないジョブが、それでもガードが応答することを示します。

プラットフォームラベルとメモの機能も同じ状況です。Linux では本物の Secret Service に
対して実行され（`secret-tool` と属性スキーマの互換性チェックを含む）、macOS と
Windows についてはプラットフォームコードの他の部分と同様にクロスターゲットで型検査
されていますが、本物の Keychain や資格情報マネージャーに対して実際に実行されるのは、
それらの CI ジョブが動いたときです。

これまでに起動した最初の CI 実行は、プラットフォームコードに本物のバグを 3 つ見つけ
ました。いずれも `cfg` で囲まれているため Linux ビルドでは型検査されない経路にありました。
型を間違えた Win32 引数が 2 つ、フィルターと全資格情報フラグを同時に渡していた
（この 2 つは同時指定できません）`CredEnumerateW` の呼び出しが 1 つ、そして
`ERROR_NOT_FOUND` を、実際に返ってくる `HRESULT` ではなく生の Win32 コードと比較して
いた箇所が 1 つ — これにより、秘密情報が見つからない場合とストアが空の場合の両方が、
生のエラーとして表に出ていました。lint ジョブは現在、Linux から Windows と macOS の
ターゲットをクロスチェックするので、この種の誤りが二度とプラットフォームランナーへ
届くことはありません。

カバーされて *いない* もの: ロック処理のテストはオプトインです
（`MASKRUN_LOCK_TESTS=1`）。実際のデスクトップで使い捨ての
`gnome-keyring-daemon` を起動すると、ユーザーにキーリングの作成を促す画面が出て、
しかもテストより長く生き残ってしまうためです。

## コマンド

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

`import` は `VITE_`、`NEXT_PUBLIC_`、`PUBLIC_`、`REACT_APP_`、`NUXT_PUBLIC_`、
`EXPO_PUBLIC_`、`GATSBY_` の各変数を飛ばします。これらはクライアントバンドルに
コンパイルされてすべての訪問者に配布されるため、秘密情報ではなく設定であり、移して
も何の得もないからです。`--all` でこの動作を上書きできます。

### プラットフォームラベルとメモ

秘密情報がひと握りを超えると、平坦な名前だけでは各々が何のためのものか思い出せなく
なります。特に、あるプラットフォームに鍵が複数あり、違いが名前ではなくスコープに
ある場合はなおさらです。`--for` は秘密情報にプラットフォーム／サービスのタグを
付け、`--note` は短い自由記述の説明を加えます。

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

どちらのフラグも任意で、まだどちらも付いていない秘密情報に適用できます。`label` は
既存の秘密情報に後からタグを付け直します。どちらのコマンドでも、`--for ""`（または
`--note ""`）を渡すとそのフィールドが消え、フラグを完全に省略すれば既存の値はその
ままです。秘密情報の値に `put` で上書きしても、ラベルが黙って落ちることはありません。
**プラットフォームとメモは秘密情報ではありません。** マスクされずに保存され、AI
エージェントのセッションからも見え、`list` と `status` に表示されます。`--note` に
秘密情報の値を入れないでください。200 文字までで、改行は含められません。

引数なしの `maskrun` は、本物の端末では [対話ビュー](#interactive-view) を開きます。
パイプされている場合（`maskrun | cat`）、非対話の場合、あるいはエージェントセッション
の場合は、代わりに短い概要を表示します。マニフェストの状態と、上に示した
グループ分けされた秘密情報の一覧です。コマンドの全体像を頭に入れておく必要が
なくなります。

### シェル補完

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # そのあと compinit の前に `fpath+=~/.zfunc`
```

フラグやサブコマンドに加えて、`maskrun get`/`rm`/`label` は実際の秘密情報の名前を
補完します。fish は標準でこれを行います（このフラグが存在する理由でもある
`maskrun list --plain` を使います）。bash と zsh は静的な補完のみです。

## 環境変数

| 変数 | 効果 |
|---|---|
| `MASKRUN_MASK` | `1` で常にマスク、`0` で決してマスクしない |
| `MASKRUN_AGENT` | `1` でエージェントセッション扱い、`0` で人間扱い |
| `MASKRUN_BACKEND` | バックエンドを強制する |
| `MASKRUN_ALLOW_READ` | `1` でエージェントセッションでも `get`/`import` を再び有効にする |

エージェントセッションは `CLAUDECODE`、`CLAUDE_CODE_ENTRYPOINT`、`AI_AGENT`、
`AIDER_CHAT`、`CURSOR_AGENT`、`OPENAI_CODEX`、`GEMINI_CLI`、`REPLIT_AGENT` から
検出されます。それ以外のものについては、ハーネス側で `MASKRUN_AGENT=1` を設定して
ください。

## 対応プラットフォーム

- **Linux** — glibc 2.35 以上（リリースバイナリは Ubuntu 22.04 でビルド）。CI で
  稼働中の Secret Service に対してテスト済み。
- **macOS** — Intel と Apple Silicon。CI で本物の Keychain に対してテスト済み。
- **Windows** — x86_64。CI で本物の資格情報マネージャーに対してテスト済み。

## 先行事例

[`envchain`](https://github.com/sorah/envchain) は何年も前から秘密情報をキーチェーンに
置き、環境変数へ注入してきました。本ツールの `run` にあたる半分の直接の祖先です。
[`direnv`](https://direnv.net/) はディレクトリごとの環境を管理し、
[`sops`](https://github.com/getsops/sops) と
[`dotenvx`](https://github.com/dotenvx/dotenvx) はリポジトリ内で秘密情報を保存時に
暗号化し、[`aws-vault`](https://github.com/99designs/aws-vault) は 1 つのプロバイダー
向けにキーチェーン周りの手順を引き受けます。

maskrun が加えるのは、エージェントに向き合う半分です。子プロセスの出力をマスクする
ことと、値を漏らしてしまうコマンドに対してハーネスが強制するガードを置くこと。AI
エージェントと一緒に作業していないなら、`envchain` だけで十分かもしれません。

## 開発

```bash
cargo test -- --test-threads=1   # シングルスレッド: バックエンドは本物のキーリング状態を共有する
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # 同じテスト実行
make lint                        # fmt --check + clippy
```

キーリングのテストは、到達可能なバックエンドがない場合に自身をスキップするので、
素のコンテナでもガードのテストは実行されます。

テストスイート内の秘密情報の値はランダムに生成され、決して出力されません。
アサーションは不在か存在かを確認するだけで、ログに出た値との一致を確認することは
決してありません。

コントリビューションを歓迎します。バックエンドの追加は `Backend` トレイトの
`put`/`get`/`delete`/`list` を実装することであり、ハーネスの追加は
`integrations/` 配下にエントリを 1 つ置くことです。

## ライセンス

Copyright (C) 2026 Furkan Akyol.

maskrun はフリーソフトウェアです。Free Software Foundation が公開する GNU General
Public License のバージョン 3、またはそれ以降の任意のバージョンの条項に基づいて、
再配布および改変できます。いかなる保証も付きません。完全な条項は
[LICENSE](LICENSE) を参照してください。

実務上の帰結として、改変した maskrun を配布する場合 — ソースとして、バイナリとして、
あるいは製品の中に含めて — 配布先に対して、改変後のソースを同じライセンスのもとで
入手できるようにしなければなりません。
