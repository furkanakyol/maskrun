[English](README.md) · [简体中文](README.zh-CN.md) · [Español](README.es.md) · **Português (BR)** · [Русский](README.ru.md) · [日本語](README.ja.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [Türkçe](README.tr.md)

> Esta é uma tradução do README em inglês. Em caso de divergência, vale a
> [versão em inglês](README.md).

# maskrun

Execute comandos com os segredos do chaveiro do seu sistema operacional — e
mantenha os valores fora do contexto do seu agente de código com IA.

```bash
maskrun put myapp-database-url        # guardado no chaveiro, nunca em disco
maskrun run -- npm run dev            # injetado no processo filho, mascarado na saída
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

Um único binário, sem dependências de execução, sem daemon. Linux, macOS e
Windows.

---

## O problema

Tirar os segredos dos arquivos `.env` e colocá-los no chaveiro do sistema é a
metade fácil. A metade difícil aparece assim que um agente de IA passa a
dirigir o seu shell.

Injetar um segredo em um processo filho impede o agente de *ler o cofre*. Não
faz nada contra o valor *voltando para fora do processo*:

- um servidor de desenvolvimento imprime a string de conexão ao subir
- `curl -v` ecoa o cabeçalho `Authorization`
- um stack trace carrega o DSN
- `psql` cita a URL à qual não conseguiu se conectar

Qualquer um desses coloca o segredo na transcrição, onde ele passa a fazer
parte da conversa, do histórico do terminal e de onde quer que essa transcrição
seja armazenada.

O maskrun fecha esse caminho, e os que ficam ao lado dele.

## Instalação

**Linux / macOS** — baixa um binário pré-compilado, verifica o checksum, sem
precisar de compilador:

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**Com uma toolchain Rust:**

```bash
cargo install maskrun
```

**A partir do código-fonte:**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# binário em target/release/maskrun
```

## Início rápido

```bash
# 1. mova um .env existente para o chaveiro — olhe antes de pular
maskrun import .env --dry-run
maskrun import .env

# 2. confira se todo nome que o manifesto pede está mesmo presente
maskrun status

# 3. rode sua aplicação sem nenhum .env em disco
maskrun run -- npm run dev

# só agora apague o .env
```

O `import` escreve um manifesto `.maskrun` ao lado do seu código:

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**Esse arquivo guarda nomes, não valores. Faça commit dele.** É o
`.env.example` que de fato faz alguma coisa: `maskrun status` diz a quem acabou
de entrar no time exatamente quais segredos estão faltando.

## Como funciona

Três mecanismos, para três necessidades diferentes.

| Necessidade | Mecanismo | O que o agente vê |
|---|---|---|
| Um servidor de desenvolvimento, uma migração ou uma CLI que precisa de `DATABASE_URL` | `maskrun run` injeta no processo filho | saída mascarada (`<masked:VAR>`) |
| O valor em si — rotacionar uma chave, colar num painel | você, no seu próprio terminal | nada; o freio recusa |
| Saber *quais* segredos existem | `maskrun list`, `maskrun status` | só nomes, nunca valores |

### Injeção, não leitura

Os valores vivem no chaveiro. O `maskrun run` resolve o manifesto, entrega os
valores a exatamente um processo filho e não grava nada em disco. Se faltar
algum segredo, ele se recusa a iniciar em vez de subir sua aplicação com meio
ambiente.

### Mascaramento da saída

`run` e `exec` passam a stdout e a stderr do filho por um filtro que substitui
os valores dos segredos por `<masked:VAR>`.

- Os valores chegam ao filtro pelo **ambiente dele**, nunca por argv — argv é
  legível via `/proc/<pid>/cmdline`.
- Mascara o valor bruto e também suas grafias em base64, codificada para URL e
  escapada com barras invertidas.
- Pega um valor **partido entre duas escritas**, de modo que um segredo em cima
  de uma fronteira de flush não escapa.
- Deixa passar linhas completas na hora, para que a saída de um servidor de
  desenvolvimento não fique em buffer enquanto você assiste.
- Recusa-se a mascarar valores com menos de 6 caracteres, e diz isso: mascarar
  `5432` corromperia todo número sem relação na saída.
- Código de saída, stdin e Ctrl-C chegam ao filho sem alteração.

Ligado por padrão em uma sessão de agente de IA e sempre que a stdout não for
um terminal; desligado no seu próprio terminal interativo, para as cores
sobreviverem. `--raw` força desligado, `--mask` força ligado.

### O freio do agente

```bash
maskrun install-guard        # registra um hook PreToolUse para o Claude Code
```

A graça de um hook é que **quem o aplica é o harness, não o modelo**. Uma regra
escrita num prompt é aplicada pelo modelo, então o modelo consegue se convencer
a contorná-la. Este aqui não dá para contornar na conversa.

Ele recusa os comandos que colocariam um valor na transcrição:

- `maskrun get`, `maskrun import`
- `secret-tool lookup/search`, `security find-generic-password`
- `--raw`, `MASKRUN_MASK=0`, `MASKRUN_ALLOW_READ=1` (desligar o mascaramento)
- `/proc/<pid>/environ`
- um `env` / `printenv` pelado, `Get-ChildItem Env:`
- ler um `.env` / `.envrc`, seja pelo Bash, seja pelas ferramentas de arquivo
  do agente

E deixa o trabalho normal em paz: `maskrun run`, `env VAR=x cmd`,
`cat .env.example`, `cat .maskrun`, `printenv PATH`, `ls -la .env`, `rm .env`.

O `maskrun install-guard` mescla com o seu `settings.json` existente, faz backup
antes, é idempotente, recusa-se a tocar em um arquivo que não seja JSON válido,
e `--remove` desfaz tudo. Outros harnesses: rode `maskrun hook` como hook de
pré-ferramenta, recebendo a chamada em JSON pela stdin — veja
[integrations/claude-code](integrations/claude-code/).

A própria CLI aplica as mesmas recusas, então um agente rodando em um harness
sem suporte a hooks continua sem conseguir fazer `maskrun get`.

### Orientar um agente que não tem mecanismo de hook

```bash
maskrun install-rules        # escreve um bloco curto em AGENTS.md/CLAUDE.md/regras .cursor
```

O `install-guard` só funciona onde o harness roda um hook PreToolUse por você.
Em outros lugares, não há nada aplicando coisa alguma — então o `install-rules`
escreve um bloco curto e demarcado naquele dos arquivos `AGENTS.md`,
`CLAUDE.md` ou `.cursor/rules/` que já existir no projeto (criando `AGENTS.md`
se nenhum existir), dizendo ao agente como usar o maskrun ali. **Isso é
orientação, não imposição**: um modelo pode ignorar, do mesmo jeito que pode
ignorar qualquer outra instrução. É um plano B para harnesses que o
`install-guard` não alcança, não um substituto dele.

O bloco é delimitado pelos marcadores
`<!-- maskrun:start -->`/`<!-- maskrun:end -->`, faz backup do arquivo antes, é
idempotente (uma segunda execução atualiza no lugar em vez de duplicar) e
`--remove` o retira sem mexer no resto do arquivo. Com um manifesto `.maskrun`
presente, o bloco lista os nomes de variáveis que o projeto de fato espera;
`--file <path>` mira direto em um arquivo, pulando a descoberta.

<a id="interactive-view"></a>

### A visão interativa

```bash
maskrun            # no seu próprio terminal, sem argumentos
```

Um terminal de verdade com mais nada na linha de comando abre uma visão
navegável pelas setas: plataformas à esquerda, os segredos daquela plataforma e
o detalhe do que estiver selecionado (plataforma, nota, valor) à direita.

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

Os valores ficam mascarados (`••••••••••••••••`) até você apertar `v`, e um
valor só é lido do chaveiro naquele instante — percorrer a lista nunca encosta
nele. Mudar para outro segredo mascara de novo automaticamente. `e` edita
plataforma, nota e valor no lugar (deixar em branco mantém o atual); `d` pede
confirmação pelo nome (`delete 'name'? [y/N]`) e só um `y`/`Y` literal segue em
frente — qualquer outra tecla, Enter incluído, cancela. `y` copia o valor para
a área de transferência.

Ela roda em um [buffer de tela
alternativo](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer):
nada do que ela desenha — nem um valor revelado — vai parar no histórico do seu
terminal, ao contrário da saída do `maskrun get` hoje. Essa é uma melhoria real,
independente de tudo o que vem a seguir.

Como todo outro caminho que toca em valores, ela **nunca abre numa sessão de
agente de IA** — com a saída redirecionada ou não, uma sessão de agente
detectada sempre recebe o mesmo resumo só-com-nomes que um `maskrun` pelado
imprime de forma não interativa. Ela também recusa abrir num terminal menor que
60x15, e diz o porquê antes de cair nesse resumo.

#### Cópia para a área de transferência

`y` copia o segredo atual para a área de transferência, já que revelar um valor
que você não pode colar em lugar nenhum só empurra o problema — você acabaria
redigitando ou selecionando com o mouse, os dois piores. Três limites embutidos:

- **Limpa sozinha depois de 45 segundos.** A TUI mostra uma contagem regressiva
  ao vivo assim que algo é copiado; `c` limpa na hora em vez de esperar, e sair
  da TUI com algo ainda copiado também limpa. Ao limpar, restaura o que estava
  na área de transferência antes da cópia, ou a esvazia se não havia nada.
- **Uma dica de "não registrar" é enviada em todas as plataformas**, via
  `exclude_from_history` do arboard: o tipo MIME `x-kde-passwordManagerHint` do
  KDE no Linux, a convenção comunitária `org.nspasteboard.ConcealedType` no
  macOS e o formato nativo `CanIncludeInClipboardHistory` no Windows.
  Verificado aqui contra um Klipper real: uma cópia comum aparece no histórico
  dele, uma marcada não, e o Klipper nem informa a marcada como conteúdo
  *atual* da área de transferência. Cada uma dessas é uma dica que uma
  ferramenta específica escolhe respeitar, não algo que o maskrun imponha —
  veja abaixo.
- Ela **nunca abre numa sessão de agente**, o mesmo portão do resto da TUI.

**O que isso não resolve:** qualquer ferramenta de histórico de área de
transferência que não procure essa dica (a do GNOME, a maioria das configurações
Wayland fora do KDE e — observado diretamente enquanto isto era construído —
até o próprio Klipper do KDE quando ele não está observando o transporte de área
de transferência que o seu compositor usa) continua registrando o valor de forma
permanente, e a limpeza automática de 45 segundos não faz nada contra essa cópia
depois que ela está num arquivo de histórico. Trate a cópia para a área de
transferência do mesmo jeito que `/proc/<pid>/environ` e que um humano colando
um valor sem máscara na mão, abaixo: um limite real, não um problema resolvido.

## O que isto não é

**O maskrun não é uma fronteira de segurança.** É endurecimento contra
acidentes, e não deveria ser vendido a você — nem por você — como algo além
disso.

- **Um chaveiro resolve armazenamento, não acesso.** Todo processo rodando com
  o seu usuário pode chamar `secret-tool lookup` ou
  `security find-generic-password`, inclusive o seu agente. O freio aumenta o
  custo de fazer isso por acidente; não torna impossível. O mesmo vale para
  você: se uma pessoa cola um valor sem máscara na transcrição na mão, nenhuma
  ferramenta depois daquela tecla consegue pegá-lo.
- **Código que o freio não consegue ler.** Corpos de heredoc e arquivos de
  script são tratados como dados, não como comandos — de propósito, porque
  analisá-los produzia falsos positivos. Então um script que lê o `.env` a
  partir do próprio código-fonte passa. Um casador de padrões nunca pega código
  arbitrário.
- **Ambiente do processo.** Enquanto o `maskrun run` está rodando, o ambiente
  do filho dele é legível via `/proc/<pid>/environ`. O freio bloqueia esse
  caminho diretamente, mas a injeção por ambiente tem esse formato por projeto.
- **A limpeza automática não desfaz a cópia para a área de transferência (`y`
  na visão interativa).** O tempo de 45 segundos esvazia *a área de
  transferência*; não faz nada contra uma ferramenta de histórico que já
  registrou o valor de forma permanente antes de ele disparar. O maskrun marca
  a cópia para o Klipper do KDE pular — isso é real e verificado, mas nem toda
  ferramenta que guarda histórico respeita essa marcação, e não se sabe que o
  histórico de área de transferência do GNOME, a maioria das configurações
  Wayland fora do KDE e o Histórico da Área de Transferência do Windows o
  façam. Mesma prateleira do `/proc/<pid>/environ` acima e do humano colando um
  valor sem máscara na mão abaixo: um limite dito com todas as letras, não
  resolvido no silêncio.
- **`ps` durante uma escrita — fechado.** No macOS, o valor chegava ao
  `security` como argumento de linha de comando, brevemente visível na lista de
  processos via `ps`. Esse caminho acabou: o maskrun agora chama direto a API
  generic-password do Security.framework, então o valor nunca vira um argv que o
  sistema tenha de expor a alguém.
- **Casamento por segmento de pipeline, não um analisador de shell.** O freio
  avalia cada segmento de um pipeline isoladamente, e é isso que deixa
  `sed 's/maskrun get/x/' notes.md` passar — esse texto nunca invoca
  `maskrun get`, apenas o menciona. O mesmo escopo significa que o freio não
  segue um comando através de indireção: `echo 'maskrun get x' | sh` é lido como
  um `echo`, não como o comando que o `sh` acaba rodando.

Se você quer uma fronteira de verdade, o shell do agente precisa rodar em algum
lugar que não alcance o chaveiro de jeito nenhum — sem socket de sessão D-Bus no
Linux, numa conta de usuário separada, ou em um contêiner. Aí o maskrun funciona
a partir do seu terminal e não do terminal do agente. O raio de impacto se
gerencia melhor no provedor: uma chave separada por ferramenta, tetos de gasto,
rotação e credenciais de curta duração onde existirem.

## Backends

| Plataforma | Nome do backend | Armazenamento |
|---|---|---|
| Linux | `secret-service` | o Secret Service do libsecret, direto por D-Bus (crate `dbus-secret-service`) — gnome-keyring, KWallet, KeePassXC |
| macOS | `keychain` | chaveiro de login via a API generic-password do Security.framework (crate `security-framework`) |
| Windows | `dpapi` | Gerenciador de Credenciais do Windows (crate `windows`, `Win32_Security_Credentials`) — o nome vem do antigo backend de arquivos DPAPI, mantido por compatibilidade de override; o armazenamento por baixo não é mais de arquivos DPAPI |

Versões anteriores chamavam `secret-tool` no Linux e `security` no macOS como
subprocesso a cada operação, e implementavam à mão o armazenamento em arquivos
DPAPI no Windows. Os três agora passam por uma ligação de biblioteca com a API
da plataforma para `put`/`get`/`delete`, em vez de um subprocesso ou de
criptografia escrita à mão — sem necessidade de instalar `secret-tool` no
Linux, e no macOS o valor do segredo não vira mais um argumento de linha de
comando. (No macOS, o `list` ainda chama `security dump-keychain` para enumerar
nomes — essa chamada não recebe nenhum valor de segredo como argumento, então
nada de novo é exposto; o `security-framework` não tem uma chamada de enumeração
restrita ao serviço para substituí-la.)

O armazenamento no Linux mantém o esquema que
`secret-tool lookup service maskrun name <name>` espera (atributos `service` +
`name`), então um segredo guardado pelo maskrun continua legível pela CLI padrão
se você precisar conferir na mão — o maskrun apenas não depende mais de essa CLI
estar instalada.

### Onde ficam o rótulo de plataforma e a nota

| Plataforma | Onde |
|---|---|
| Linux | Dois atributos extras do Secret Service, `platform` e `note`, ao lado dos `service`/`name` já existentes — o `secret-tool lookup` só casa com os atributos que você passar, então esses viajam junto sem afetar nada que já leia `service`+`name`. |
| macOS | Os campos `kSecAttrDescription` (plataforma) e `kSecAttrComment` (nota) da generic password, escritos e lidos pelas APIs de busca por atributos e de atualização somente-de-atributos do `security-framework` — o valor guardado em si nunca é tocado numa reetiquetagem. |
| Windows | Ambos codificados no único campo `CREDENTIALW.Comment` que o Gerenciador de Credenciais oferece (`platform=<p><US>note=<n>`, `<US>` = U+001F): aqui não há um repositório de atributos separado, e reetiquetar obriga a reescrever a entrada inteira (valor incluído), porque o `CredWriteW` não tem chamada de atualização parcial. |

Force a detecção com `MASKRUN_BACKEND=secret-service|keychain|dpapi`.

### O que está de fato verificado

Os três backends são exercitados contra um chaveiro real na CI: Secret Service
no Linux, Keychain no macOS, Gerenciador de Credenciais no Windows. Os testes
de chaveiro gravam e leem de volta um segredo real em vez de simular o backend,
e um job sem chaveiro nenhum instalado prova que o freio continua respondendo.

Com o rótulo de plataforma e a nota é a mesma história: exercitados contra um
Secret Service real no Linux (incluindo a checagem de compatibilidade do
`secret-tool` com o esquema de atributos), verificados por tipos em compilação
cruzada para macOS e Windows como o resto do código de plataforma, mas de fato
executados contra um Keychain ou um Gerenciador de Credenciais reais só quando
esses jobs de CI rodarem.

A primeiríssima execução de CI que chegou a começar encontrou três bugs reais no
código de plataforma, todos em caminhos que uma compilação Linux nunca verifica
por tipos, por estarem atrás de `cfg`: dois argumentos Win32 com tipo errado,
uma chamada a `CredEnumerateW` passando ao mesmo tempo um filtro e a flag de
todas-as-credenciais (inválidos juntos), e uma comparação de `ERROR_NOT_FOUND`
contra o código Win32 cru em vez do `HRESULT` que de fato chega — o que fazia um
segredo ausente e um repositório vazio aparecerem ambos como erro cru. O job de
lint agora checa os alvos Windows e macOS a partir do Linux, para que essa
classe de erro não chegue mais a um runner de plataforma.

O que *não* está coberto: os testes de tratamento de bloqueio são opcionais
(`MASKRUN_LOCK_TESTS=1`), porque subir um `gnome-keyring-daemon` descartável num
desktop real pede ao usuário que crie um chaveiro e sobrevive ao teste.

## Comandos

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

O `import` pula as variáveis `VITE_`, `NEXT_PUBLIC_`, `PUBLIC_`, `REACT_APP_`,
`NUXT_PUBLIC_`, `EXPO_PUBLIC_` e `GATSBY_`: elas são compiladas no seu bundle de
cliente e entregues a todo visitante, ou seja, são configuração e não segredos,
e movê-las não compra nada. `--all` sobrepõe isso.

### Rótulos de plataforma e notas

Passando de um punhado de segredos, um nome solto não basta para lembrar para
que serve cada um — ainda mais quando uma plataforma tem mais de uma chave e o
que as diferencia é o escopo, não o nome. `--for` marca um segredo com uma
plataforma/serviço; `--note` acrescenta uma descrição curta em texto livre:

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

As duas flags são opcionais e se aplicam a segredos que ainda não têm nenhuma
delas. O `label` reetiqueta um segredo existente depois do fato; em qualquer um
dos dois comandos, passar `--for ""` (ou `--note ""`) limpa aquele campo, e
omitir a flag por completo deixa o valor existente em paz — dar `put` por cima
do valor de um segredo nunca derruba o rótulo dele em silêncio. **A plataforma e
a nota não são segredos**: são guardadas sem máscara, ficam visíveis em uma
sessão de agente de IA, e aparecem no `list` e no `status`. Não coloque um valor
secreto em `--note`; ele é limitado a 200 caracteres e não pode conter quebra de
linha.

O `maskrun` sem argumentos abre [a visão interativa](#interactive-view) num
terminal de verdade; redirecionado (`maskrun | cat`), não interativo, ou dentro
de uma sessão de agente, ele imprime um resumo curto: o estado do manifesto e a
lista agrupada de segredos acima, para você não precisar guardar toda a
superfície de comandos na cabeça.

### Autocompletar do shell

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # depois `fpath+=~/.zfunc` antes do compinit
```

Além de flags e subcomandos, `maskrun get`/`rm`/`label` completam nomes reais de
segredos — o fish faz isso de fábrica (via o mesmo `maskrun list --plain` para o
qual essa flag existe); bash e zsh ficam só com os completadores estáticos.

## Variáveis de ambiente

| Variável | Efeito |
|---|---|
| `MASKRUN_MASK` | `1` sempre mascarar, `0` nunca mascarar |
| `MASKRUN_AGENT` | `1` tratar como sessão de agente, `0` tratar como humano |
| `MASKRUN_BACKEND` | forçar um backend |
| `MASKRUN_ALLOW_READ` | `1` reativa `get`/`import` numa sessão de agente |

Sessões de agente são detectadas por `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
`AI_AGENT`, `AIDER_CHAT`, `CURSOR_AGENT`, `OPENAI_CODEX`, `GEMINI_CLI` e
`REPLIT_AGENT`. Para qualquer outro caso, defina `MASKRUN_AGENT=1` no harness.

## Suporte de plataformas

- **Linux** — glibc 2.35 ou mais novo (os binários de release são compilados no
  Ubuntu 22.04). Testado na CI contra um Secret Service ativo.
- **macOS** — Intel e Apple Silicon. Testado na CI contra um Keychain real.
- **Windows** — x86_64. Testado na CI contra o Gerenciador de Credenciais real.

## Trabalhos anteriores

O [`envchain`](https://github.com/sorah/envchain) já colocava segredos no
chaveiro e os injetava no ambiente anos atrás, e é o ancestral direto da metade
`run` desta ferramenta. O [`direnv`](https://direnv.net/) gerencia ambientes por
diretório, [`sops`](https://github.com/getsops/sops) e
[`dotenvx`](https://github.com/dotenvx/dotenvx) cifram segredos em repouso
dentro do repositório, e o
[`aws-vault`](https://github.com/99designs/aws-vault) faz a dança do chaveiro
para um provedor.

O que o maskrun acrescenta é a metade voltada para o agente: mascarar a saída do
processo filho, e um freio aplicado pelo harness sobre os comandos que
vazariam um valor. Se você não trabalha com um agente de IA, talvez o
`envchain` já baste.

## Desenvolvimento

```bash
cargo test -- --test-threads=1   # thread única: os backends compartilham o estado real do chaveiro
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # a mesma rodada de testes
make lint                        # fmt --check + clippy
```

Os testes de chaveiro se pulam sozinhos quando nenhum backend está acessível,
para que os testes do freio ainda rodem num contêiner pelado.

Os valores secretos da suíte de testes são gerados aleatoriamente e nunca
impressos — as asserções checam ausência ou presença, nunca igualdade contra um
valor registrado em log.

Contribuições são bem-vindas. Acrescentar um backend significa implementar
`put`/`get`/`delete`/`list` do trait `Backend`; acrescentar um harness significa
uma entrada em `integrations/`.

## Licença

Copyright (C) 2026 Furkan Akyol.

O maskrun é software livre: você pode redistribuí-lo e modificá-lo sob os termos
da Licença Pública Geral GNU, versão 3 ou qualquer versão posterior, publicada
pela Free Software Foundation. Ele vem sem nenhuma garantia. Veja
[LICENSE](LICENSE) para os termos completos.

Uma consequência prática: se você distribuir um maskrun modificado — como
código-fonte, como binário ou dentro de um produto — precisa disponibilizar seu
código-fonte modificado a quem você o distribuir, sob esta mesma licença.
