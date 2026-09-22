[English](README.md) · [简体中文](README.zh-CN.md) · [Español](README.es.md) · [Português (BR)](README.pt-BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · **Français** · [Deutsch](README.de.md) · [Türkçe](README.tr.md)

> Ceci est une traduction du README anglais. En cas de divergence, la
> [version anglaise](README.md) fait foi.

# maskrun

Exécutez des commandes avec les secrets de votre trousseau système — et gardez
les valeurs hors du contexte de votre agent de code IA.

```bash
maskrun put myapp-database-url        # stocké dans le trousseau, jamais sur le disque
maskrun run -- npm run dev            # injecté dans le processus fils, masqué en sortie
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

Un seul binaire, aucune dépendance d'exécution, aucun démon. Linux, macOS et
Windows.

---

## Le problème

Déplacer les secrets des fichiers `.env` vers le trousseau système, c'est la
moitié facile. La moitié difficile apparaît dès qu'un agent IA pilote votre
shell.

Injecter un secret dans un processus fils empêche l'agent de *lire le coffre*.
Cela ne fait rien contre la valeur qui *ressort du processus* :

- un serveur de dev affiche sa chaîne de connexion au démarrage
- `curl -v` renvoie l'en-tête `Authorization`
- une trace d'appels transporte le DSN
- `psql` cite l'URL à laquelle il n'a pas réussi à se connecter

N'importe lequel de ces cas place le secret dans la transcription, où il fait
désormais partie de la conversation, de l'historique du terminal et de partout
où cette transcription est stockée.

maskrun ferme ce chemin, et ceux qui l'entourent.

## Installation

**Linux / macOS** — télécharge un binaire précompilé, vérifie sa somme de
contrôle, aucun compilateur nécessaire :

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**Depuis une chaîne d'outils Rust :**

```bash
cargo install maskrun
```

**Depuis les sources :**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# binaire dans target/release/maskrun
```

## Démarrage rapide

```bash
# 1. déplacer un .env existant dans le trousseau — regardez avant de sauter
maskrun import .env --dry-run
maskrun import .env

# 2. vérifier que chaque nom attendu par le manifeste est bien présent
maskrun status

# 3. lancer votre application sans .env sur le disque
maskrun run -- npm run dev

# seulement maintenant, supprimez le .env
```

`import` écrit un manifeste `.maskrun` à côté de votre code :

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**Ce fichier contient des noms, pas des valeurs. Versionnez-le.** C'est le
`.env.example` qui fait réellement quelque chose : `maskrun status` indique à
un nouvel équipier exactement quels secrets lui manquent.

## Comment ça marche

Trois mécanismes, pour trois besoins différents.

| Besoin | Mécanisme | Ce que voit l'agent |
|---|---|---|
| Un serveur de dev, une migration ou une CLI qui a besoin de `DATABASE_URL` | `maskrun run` injecte dans le processus fils | sortie masquée (`<masked:VAR>`) |
| La valeur elle-même — rotation d'une clé, collage dans un tableau de bord | vous, dans votre propre terminal | rien ; le frein refuse |
| Savoir *quels* secrets existent | `maskrun list`, `maskrun status` | des noms seulement, jamais des valeurs |

### Injection, pas lecture

Les valeurs vivent dans le trousseau. `maskrun run` résout le manifeste, remet
les valeurs à exactement un processus fils, et n'écrit rien sur le disque. S'il
manque un secret, il refuse de démarrer plutôt que de lancer votre application
avec un environnement incomplet.

### Masquage de la sortie

`run` et `exec` font passer stdout et stderr du processus fils par un filtre
qui remplace les valeurs des secrets par `<masked:VAR>`.

- Les valeurs atteignent le filtre par **son environnement**, jamais par argv —
  argv est lisible via `/proc/<pid>/cmdline`.
- Masque la valeur brute ainsi que ses écritures en base64, encodée pour URL et
  échappée par barres obliques inverses.
- Attrape une valeur **répartie sur deux écritures**, pour qu'un secret à
  cheval sur une frontière de vidage ne passe pas au travers.
- Laisse passer immédiatement les lignes complètes, pour que la sortie d'un
  serveur de dev ne soit pas mise en tampon pendant que vous la regardez.
- Refuse de masquer les valeurs de moins de 6 caractères, et le dit : masquer
  `5432` corromprait tous les nombres sans rapport dans la sortie.
- Le code de sortie, stdin et Ctrl-C parviennent au processus fils sans
  changement.

Actif par défaut dans une session d'agent IA et chaque fois que stdout n'est
pas un terminal ; inactif dans votre propre terminal interactif pour préserver
les couleurs. `--raw` force l'arrêt, `--mask` force l'activation.

### Le frein de l'agent

```bash
maskrun install-guard        # enregistre un hook PreToolUse pour Claude Code
```

Tout l'intérêt d'un hook est que **c'est le harnais qui l'applique, pas le
modèle**. Une règle écrite dans un prompt est appliquée par le modèle, donc le
modèle peut s'en dissuader lui-même. Ici, impossible.

Il refuse les commandes qui mettraient une valeur dans la transcription :

- `maskrun get`, `maskrun import`
- `secret-tool lookup/search`, `security find-generic-password`
- `--raw`, `MASKRUN_MASK=0`, `MASKRUN_ALLOW_READ=1` (désactiver le masquage)
- `/proc/<pid>/environ`
- un `env` / `printenv` nu, `Get-ChildItem Env:`
- la lecture d'un `.env` / `.envrc`, que ce soit via Bash ou via les outils de
  fichiers de l'agent

Et il laisse le travail normal tranquille : `maskrun run`, `env VAR=x cmd`,
`cat .env.example`, `cat .maskrun`, `printenv PATH`, `ls -la .env`, `rm .env`.

`maskrun install-guard` fusionne dans votre `settings.json` existant, en fait
d'abord une sauvegarde, est idempotent, refuse de toucher à un fichier qui
n'est pas du JSON valide, et `--remove` annule l'opération. Autres harnais :
lancez `maskrun hook` comme hook pré-outil, qui reçoit l'appel d'outil en JSON
sur stdin — voir [integrations/claude-code](integrations/claude-code/).

La CLI applique elle-même les mêmes refus, de sorte qu'un agent tournant dans
un harnais sans support de hooks ne peut toujours pas faire `maskrun get`.

### Guider un agent dépourvu de mécanisme de hook

```bash
maskrun install-rules        # écrit un court bloc dans AGENTS.md/CLAUDE.md/les règles .cursor
```

`install-guard` ne fonctionne que là où le harnais exécute un hook PreToolUse
pour vous. Ailleurs, rien n'applique quoi que ce soit — `install-rules` écrit
donc un court bloc balisé dans celui de `AGENTS.md`, `CLAUDE.md` ou
`.cursor/rules/` qui existe déjà dans le projet (en créant `AGENTS.md` si aucun
n'existe), pour expliquer à l'agent comment utiliser maskrun ici. **C'est une
orientation, pas une application** : un modèle peut l'ignorer, comme il peut
ignorer n'importe quelle autre instruction. C'est un repli pour les harnais que
`install-guard` ne peut pas atteindre, pas un substitut.

Le bloc est délimité par les marqueurs
`<!-- maskrun:start -->`/`<!-- maskrun:end -->`, sauvegarde d'abord le fichier,
est idempotent (une seconde exécution le met à jour sur place au lieu de le
dupliquer), et `--remove` le retire sans toucher au reste du fichier. Si un
manifeste `.maskrun` est présent, le bloc liste les noms de variables réellement
attendus par le projet ; `--file <path>` vise directement un fichier et saute
la découverte.

<a id="interactive-view"></a>

### La vue interactive

```bash
maskrun            # dans votre propre terminal, sans arguments
```

Un vrai terminal, sans rien d'autre sur la ligne de commande, ouvre une vue au
clavier fléché : les plateformes à gauche, les secrets de cette plateforme et
le détail de celui qui est sélectionné (plateforme, note, valeur) à droite.

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

Les valeurs sont masquées (`••••••••••••••••`) jusqu'à ce que vous appuyiez sur
`v`, et une valeur n'est lue dans le trousseau qu'à cet instant — parcourir la
liste n'y touche jamais. Passer à un autre secret remasque automatiquement. `e`
modifie la plateforme, la note et la valeur sur place (laisser vide conserve
l'existant) ; `d` demande une confirmation par le nom
(`delete 'name'? [y/N]`) et seul un `y`/`Y` littéral poursuit — toute autre
touche, y compris Entrée, annule. `y` copie la valeur dans votre presse-papiers.

Elle s'exécute dans un [tampon d'écran
alternatif](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer) :
rien de ce qu'elle dessine — y compris une valeur révélée — n'atterrit jamais
dans l'historique de votre terminal, contrairement à la sortie actuelle de
`maskrun get`. C'est une amélioration réelle, indépendamment de tout ce qui
suit.

Comme tout autre chemin touchant aux valeurs, elle **ne s'ouvre jamais dans une
session d'agent IA** — sortie redirigée ou non, une session d'agent détectée
reçoit toujours le même résumé sans valeurs qu'un `maskrun` nu affiche en mode
non interactif. Elle refuse également de s'ouvrir sur un terminal plus petit que
60x15, et vous en dit la raison avant de se replier sur ce résumé.

#### Copie dans le presse-papiers

`y` copie le secret courant dans votre presse-papiers, car révéler une valeur
que vous ne pouvez ensuite coller nulle part ne fait que déplacer le problème —
vous la retaperiez ou la sélectionneriez à la souris, deux options pires. Trois
limites intégrées :

- **Effacement automatique après 45 secondes.** La TUI affiche un compte à
  rebours en direct dès que quelque chose est copié ; `c` efface
  immédiatement au lieu d'attendre, et quitter la TUI alors que quelque chose
  est encore copié efface aussi. L'effacement restaure ce qui se trouvait dans
  le presse-papiers avant la copie, ou le vide s'il n'y avait rien.
- **Un indice « ne pas enregistrer » est émis sur chaque plateforme**, via
  `exclude_from_history` d'arboard : le type MIME `x-kde-passwordManagerHint`
  de KDE sous Linux, la convention communautaire
  `org.nspasteboard.ConcealedType` sous macOS, et le format de presse-papiers
  natif `CanIncludeInClipboardHistory` sous Windows. Vérifié ici contre un vrai
  Klipper : une copie ordinaire apparaît dans son historique, une copie balisée
  non, et Klipper ne signale même pas la copie balisée comme contenu *courant*
  du presse-papiers. Chacun de ces mécanismes est un indice qu'un outil donné
  choisit de respecter, pas quelque chose que maskrun impose — voir ci-dessous.
- Elle **ne s'ouvre jamais dans une session d'agent**, même porte que pour le
  reste de la TUI.

**Ce que cela ne règle pas :** tout outil d'historique de presse-papiers qui ne
cherche pas cet indice (celui de GNOME, la plupart des configurations Wayland
hors KDE et — observé directement en construisant ceci — même le Klipper de KDE
lorsqu'il ne surveille pas le transport de presse-papiers qu'utilise votre
compositeur) enregistre toujours la valeur de façon permanente, et l'effacement
automatique à 45 secondes ne change rien à cette copie une fois qu'elle est dans
un fichier d'historique. Traitez la copie dans le presse-papiers comme
`/proc/<pid>/environ` et comme un humain qui colle une valeur non masquée à la
main, ci-dessous : une limite réelle, pas un problème résolu.

## Ce que ce n'est pas

**maskrun n'est pas une frontière de sécurité.** C'est un durcissement contre
les accidents, et cela ne devrait pas vous être vendu — ni vendu par vous —
comme quoi que ce soit de plus.

- **Un trousseau résout le stockage, pas l'accès.** Tout processus tournant
  sous votre compte peut appeler `secret-tool lookup` ou
  `security find-generic-password`, votre agent compris. Le frein augmente le
  coût de le faire par accident ; il ne le rend pas impossible. Cela vaut aussi
  pour vous : si un humain colle une valeur non masquée à la main dans la
  transcription, aucun outil en aval de cette frappe ne peut la rattraper.
- **Du code que le frein ne peut pas lire.** Les corps de heredoc et les
  fichiers de script sont traités comme des données, pas comme des commandes —
  délibérément, parce que les analyser produisait des faux positifs. Un script
  qui lit lui-même `.env` depuis son propre source passe donc. Un
  correspondeur de motifs n'attrape jamais du code arbitraire.
- **Environnement de processus.** Pendant que `maskrun run` tourne,
  l'environnement de son processus fils est lisible via `/proc/<pid>/environ`.
  Le frein bloque ce chemin directement, mais l'injection par environnement a
  cette forme par conception.
- **La copie dans le presse-papiers (`y` dans la vue interactive) n'est pas
  annulée par l'effacement automatique.** Le délai de 45 secondes vide *le
  presse-papiers* ; il ne fait rien contre un outil d'historique qui a déjà
  enregistré la valeur de façon permanente avant son déclenchement. maskrun
  balise la copie pour que le Klipper de KDE l'ignore — c'est réel et vérifié,
  mais tous les outils qui tiennent un historique ne respectent pas ce balisage,
  et l'historique de presse-papiers de GNOME, la plupart des configurations
  Wayland hors KDE et l'historique du Presse-papiers Windows ne sont pas connus
  pour le faire. Même étagère que `/proc/<pid>/environ` ci-dessus et que
  l'humain qui colle une valeur non masquée à la main ci-dessous : une limite
  énoncée franchement, pas résolue en silence.
- **`ps` pendant une écriture — fermé.** Sous macOS, la valeur parvenait
  autrefois à `security` en argument de ligne de commande, brièvement visible
  dans la liste des processus via `ps`. Ce chemin a disparu : maskrun appelle
  désormais directement l'API generic-password de Security.framework, de sorte
  que la valeur ne devient jamais un argv que l'OS doive exposer à quiconque.
- **Correspondance par segment de pipeline, pas un analyseur shell.** Le frein
  évalue chaque segment d'un pipeline séparément, et c'est ce qui laisse passer
  `sed 's/maskrun get/x/' notes.md` — ce texte n'invoque jamais `maskrun get`,
  il ne fait que le mentionner. La même portée implique que le frein ne suit pas
  une commande à travers une indirection : `echo 'maskrun get x' | sh` se lit
  comme un `echo`, pas comme la commande que `sh` finit par exécuter.

Si vous voulez une vraie frontière, le shell de l'agent doit tourner quelque
part où il ne peut pas du tout atteindre le trousseau — sans socket de session
D-Bus sous Linux, dans un compte utilisateur distinct, ou dans un conteneur.
maskrun fonctionne alors depuis votre terminal et non depuis celui de l'agent.
Le rayon d'impact se gère mieux chez le fournisseur : une clé distincte par
outil, des plafonds de dépenses, la rotation, et des identifiants de courte
durée là où ils existent.

## Backends

| Plateforme | Nom du backend | Stockage |
|---|---|---|
| Linux | `secret-service` | le Secret Service de libsecret, directement via D-Bus (crate `dbus-secret-service`) — gnome-keyring, KWallet, KeePassXC |
| macOS | `keychain` | trousseau de session via l'API generic-password de Security.framework (crate `security-framework`) |
| Windows | `dpapi` | Gestionnaire d'identifiants Windows (crate `windows`, `Win32_Security_Credentials`) — le nom est conservé de l'ancien backend à fichiers DPAPI pour la compatibilité des surcharges ; le stockage sous-jacent n'est plus constitué de fichiers DPAPI |

Les versions antérieures lançaient `secret-tool` sous Linux et `security` sous
macOS pour chaque opération, et implémentaient à la main le stockage par
fichiers DPAPI sous Windows. Les trois passent désormais par une liaison de
bibliothèque vers l'API de la plateforme pour `put`/`get`/`delete`, plutôt que
par un sous-processus ou de la cryptographie écrite à la main — aucune
installation de `secret-tool` requise sous Linux, et sous macOS la valeur du
secret ne devient plus un argument de ligne de commande. (Sous macOS, `list`
lance encore `security dump-keychain` pour énumérer les noms — cet appel ne
prend aucune valeur de secret en argument, donc rien de nouveau n'est exposé ;
`security-framework` n'offre aucun appel d'énumération limité au service pour le
remplacer.)

Le stockage Linux conserve le schéma attendu par
`secret-tool lookup service maskrun name <name>` (attributs `service` +
`name`), de sorte qu'un secret stocké par maskrun reste lisible avec la CLI
standard si vous devez vérifier à la main — maskrun lui-même ne dépend
simplement plus de l'installation de cette CLI.

### Où vivent l'étiquette de plateforme et la note

| Plateforme | Où |
|---|---|
| Linux | Deux attributs Secret Service supplémentaires, `platform` et `note`, aux côtés des `service`/`name` existants — `secret-tool lookup` ne fait correspondre que les attributs que vous lui donnez, donc ceux-ci voyagent avec sans affecter quoi que ce soit qui lit déjà `service`+`name`. |
| macOS | Les champs `kSecAttrDescription` (plateforme) et `kSecAttrComment` (note) du generic password, écrits et lus via les API de recherche par attributs et de mise à jour d'attributs seuls de `security-framework` — la valeur stockée elle-même n'est jamais touchée par un réétiquetage. |
| Windows | Les deux encodés dans l'unique champ `CREDENTIALW.Comment` offert par le Gestionnaire d'identifiants (`platform=<p><US>note=<n>`, `<US>` = U+001F) : il n'y a pas de magasin d'attributs distinct ici, et le réétiquetage doit réécrire l'entrée entière (valeur comprise) parce que `CredWriteW` n'a pas d'appel de mise à jour partielle. |

Forcez la détection avec `MASKRUN_BACKEND=secret-service|keychain|dpapi`.

### Ce qui est réellement vérifié

Les trois backends sont exercés contre un vrai trousseau en CI : Secret Service
sous Linux, Keychain sous macOS, Gestionnaire d'identifiants sous Windows. Les
tests de trousseau font l'aller-retour d'un secret réel plutôt que de simuler
le backend, et un job sans aucun trousseau installé prouve que le frein répond
quand même.

L'étiquette de plateforme et la note, c'est la même histoire : exercées contre
un vrai Secret Service sous Linux (y compris la vérification de compatibilité
`secret-tool`/schéma d'attributs), vérifiées au niveau des types en
compilation croisée pour macOS et Windows comme le reste du code de plateforme,
mais réellement exécutées contre un vrai Keychain ou Gestionnaire d'identifiants
seulement quand ces jobs CI tourneront.

La toute première exécution CI qui ait démarré a trouvé trois bogues réels dans
le code de plateforme, tous dans des chemins qu'une compilation Linux ne vérifie
jamais parce qu'ils sont derrière des `cfg` : deux arguments Win32 mal typés, un
appel à `CredEnumerateW` passant à la fois un filtre et le drapeau « tous les
identifiants » (invalides ensemble), et une comparaison d'`ERROR_NOT_FOUND`
contre le code Win32 brut au lieu du `HRESULT` qui arrive réellement — ce qui
faisait remonter un secret manquant et un magasin vide tous deux comme une
erreur brute. Le job de lint vérifie désormais en croisé les cibles Windows et
macOS depuis Linux, pour que cette classe d'erreur ne puisse plus atteindre un
runner de plateforme.

Ce qui n'est *pas* couvert : les tests de gestion du verrouillage sont optionnels
(`MASKRUN_LOCK_TESTS=1`), car lancer un `gnome-keyring-daemon` jetable sur un
vrai bureau demande à l'utilisateur de créer un trousseau et survit au test.

## Commandes

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

`import` ignore les variables `VITE_`, `NEXT_PUBLIC_`, `PUBLIC_`, `REACT_APP_`,
`NUXT_PUBLIC_`, `EXPO_PUBLIC_` et `GATSBY_` : elles sont compilées dans votre
bundle client et expédiées à chaque visiteur, ce sont donc de la configuration
et pas des secrets, et les déplacer n'apporte rien. `--all` passe outre.

### Étiquettes de plateforme et notes

Passé une poignée de secrets, un nom plat ne suffit plus à se rappeler à quoi
sert chacun — surtout quand une plateforme a plus d'une clé et que la différence
entre elles tient à la portée, pas au nom. `--for` étiquette un secret avec une
plateforme ou un service ; `--note` ajoute une courte description libre :

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

Les deux options sont facultatives et s'appliquent aux secrets qui n'ont encore
ni l'une ni l'autre. `label` réétiquette après coup un secret existant ; avec
l'une ou l'autre commande, donner `--for ""` (ou `--note ""`) vide ce champ, et
omettre complètement une option laisse la valeur existante intacte — faire un
`put` par-dessus la valeur d'un secret ne perd jamais silencieusement son
étiquette. **La plateforme et la note ne sont pas des secrets** : elles sont
stockées en clair, sont visibles dans une session d'agent IA, et sont affichées
par `list` et `status`. Ne mettez pas de valeur secrète dans `--note` ; ce champ
est limité à 200 caractères et ne peut pas contenir de saut de ligne.

`maskrun` sans arguments ouvre [la vue interactive](#interactive-view) dans un
vrai terminal ; redirigé (`maskrun | cat`), non interactif, ou dans une session
d'agent, il affiche plutôt un court aperçu : l'état du manifeste et la liste de
secrets groupée ci-dessus, pour que vous n'ayez pas à garder en tête toute la
surface de commandes.

### Complétion shell

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # puis `fpath+=~/.zfunc` avant compinit
```

Au-delà des options et des sous-commandes, `maskrun get`/`rm`/`label`
complètent de vrais noms de secrets — fish le fait nativement (via ce même
`maskrun list --plain` pour lequel cette option existe) ; bash et zsh
n'obtiennent que les complétions statiques.

## Variables d'environnement

| Variable | Effet |
|---|---|
| `MASKRUN_MASK` | `1` toujours masquer, `0` ne jamais masquer |
| `MASKRUN_AGENT` | `1` traiter comme une session d'agent, `0` comme un humain |
| `MASKRUN_BACKEND` | forcer un backend |
| `MASKRUN_ALLOW_READ` | `1` réactive `get`/`import` dans une session d'agent |

Les sessions d'agent sont détectées via `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
`AI_AGENT`, `AIDER_CHAT`, `CURSOR_AGENT`, `OPENAI_CODEX`, `GEMINI_CLI` et
`REPLIT_AGENT`. Pour tout le reste, définissez `MASKRUN_AGENT=1` dans le
harnais.

## Plateformes prises en charge

- **Linux** — glibc 2.35 ou plus récent (les binaires de release sont
  construits sur Ubuntu 22.04). Testé en CI contre un Secret Service actif.
- **macOS** — Intel et Apple Silicon. Testé en CI contre un vrai Keychain.
- **Windows** — x86_64. Testé en CI contre le vrai Gestionnaire d'identifiants.

## Travaux antérieurs

[`envchain`](https://github.com/sorah/envchain) mettait les secrets dans le
trousseau et les injectait dans l'environnement il y a des années, et c'est
l'ancêtre direct de la moitié `run` de cet outil.
[`direnv`](https://direnv.net/) gère des environnements par répertoire,
[`sops`](https://github.com/getsops/sops) et
[`dotenvx`](https://github.com/dotenvx/dotenvx) chiffrent les secrets au repos
dans le dépôt, et [`aws-vault`](https://github.com/99designs/aws-vault) fait la
danse du trousseau pour un fournisseur.

Ce que maskrun ajoute, c'est la moitié tournée vers l'agent : masquer la sortie
du processus fils, et un frein appliqué par le harnais sur les commandes qui
divulgueraient une valeur. Si vous ne travaillez pas avec un agent IA,
`envchain` vous suffira peut-être.

## Développement

```bash
cargo test -- --test-threads=1   # mono-thread : les backends partagent un vrai état de trousseau
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # même exécution de tests
make lint                        # fmt --check + clippy
```

Les tests de trousseau se sautent eux-mêmes lorsqu'aucun backend n'est
joignable, pour que les tests du frein tournent quand même dans un conteneur nu.

Les valeurs secrètes de la suite de tests sont générées aléatoirement et jamais
affichées — les assertions vérifient une absence ou une présence, jamais une
égalité avec une valeur journalisée.

Les contributions sont bienvenues. Ajouter un backend revient à implémenter
`put`/`get`/`delete`/`list` du trait `Backend` ; ajouter un harnais revient à
une entrée sous `integrations/`.

## Licence

Copyright (C) 2026 Furkan Akyol.

maskrun est un logiciel libre : vous pouvez le redistribuer et le modifier selon
les termes de la GNU General Public License, version 3 ou toute version
ultérieure, telle que publiée par la Free Software Foundation. Il est fourni
sans aucune garantie. Voir [LICENSE](LICENSE) pour les termes complets.

Une conséquence pratique : si vous distribuez un maskrun modifié — sous forme de
source, de binaire, ou à l'intérieur d'un produit — vous devez mettre votre
source modifié à disposition de ceux à qui vous le distribuez, sous cette même
licence.
