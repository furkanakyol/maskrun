[English](README.md) · [简体中文](README.zh-CN.md) · [Español](README.es.md) · [Português (BR)](README.pt-BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · **Türkçe**

> Bu, İngilizce README'nin çevirisidir. Bir uyuşmazlık durumunda
> [İngilizce sürüm](README.md) esas alınır.

# maskrun

Komutları işletim sistemi anahtarlığındaki sırlarla çalıştır — ve değerleri
yapay zekâ kodlama ajanının context'inin dışında tut.

```bash
maskrun put myapp-database-url        # anahtarlıkta saklanır, diske hiç yazılmaz
maskrun run -- npm run dev            # çocuk sürece enjekte edilir, çıktıda maskelenir
```

```
$ maskrun run -- node -e 'console.log(process.env.DATABASE_URL)'
<masked:DATABASE_URL>
```

Tek binary, çalışma zamanı bağımlılığı yok, servis yok. Linux, macOS ve Windows.

---

## Sorun

Sırları `.env` dosyalarından işletim sistemi anahtarlığına taşımak işin kolay
yarısı. Zor yarısı, kabuğunu bir yapay zekâ ajanı sürmeye başladığında ortaya
çıkıyor.

Bir sırrı çocuk sürece enjekte etmek, ajanın *kasadan okumasını* engeller.
Değerin *süreçten geri çıkmasına* dair hiçbir şey yapmaz:

- bir geliştirme sunucusu açılışta bağlantı dizesini basar
- `curl -v`, `Authorization` başlığını yankılar
- bir yığın izi DSN'i taşır
- `psql`, bağlanamadığı URL'yi tırnak içinde gösterir

Bunlardan herhangi biri sırrı transcript'e koyar; artık sır konuşmanın,
terminal geçmişinin ve o transcript'in saklandığı her yerin parçasıdır.

maskrun bu yolu ve yanındakileri kapatır.

## Kurulum

**Linux / macOS** — hazır binary indirir, checksum'ını doğrular, derleyici
gerekmez:

```bash
curl -fsSL https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/furkanakyol/maskrun/main/install.ps1 | iex
```

**Rust araç zinciriyle:**

```bash
cargo install maskrun
```

**Kaynaktan:**

```bash
git clone https://github.com/furkanakyol/maskrun
cd maskrun && cargo build --release
# binary: target/release/maskrun
```

## Hızlı başlangıç

```bash
# 1. mevcut bir .env'i anahtarlığa taşı — atlamadan önce bak
maskrun import .env --dry-run
maskrun import .env

# 2. manifest'in istediği her adın gerçekten mevcut olduğunu denetle
maskrun status

# 3. uygulamanı diskte .env olmadan çalıştır
maskrun run -- npm run dev

# .env'i ancak şimdi sil
```

`import`, kodunun yanına bir `.maskrun` manifest'i yazar:

```
DATABASE_URL=myapp-database-url
JWT_SECRET=myapp-jwt-secret
```

**Bu dosya değerleri değil, adları tutar. Commit'le.** Gerçekten bir işe
yarayan `.env.example` budur: `maskrun status`, ekibe yeni katılan birine hangi
sırların eksik olduğunu tam olarak söyler.

## Nasıl çalışır

Üç farklı ihtiyaç için üç mekanizma.

| İhtiyaç | Mekanizma | Ajanın gördüğü |
|---|---|---|
| `DATABASE_URL` isteyen bir geliştirme sunucusu, migration veya CLI | `maskrun run` çocuk sürece enjekte eder | maskelenmiş çıktı (`<masked:VAR>`) |
| Değerin kendisi — anahtar döndürme, bir panoya yapıştırma | kendi terminalinde, sen | hiçbir şey; fren reddeder |
| *Hangi* sırların var olduğunu bilmek | `maskrun list`, `maskrun status` | yalnızca adlar, asla değerler |

### Okuma değil, enjeksiyon

Değerler anahtarlıkta yaşar. `maskrun run` manifest'i çözer, değerleri tam
olarak tek bir çocuk sürece verir ve diske hiçbir şey yazmaz. Herhangi bir sır
eksikse, uygulamanı yarım bir ortamla başlatmak yerine çalışmayı reddeder.

### Çıktı maskeleme

`run` ve `exec`, çocuğun stdout ve stderr'ini, sır değerlerini `<masked:VAR>`
ile değiştiren bir süzgeçten geçirir.

- Değerler süzgece **kendi ortamı** üzerinden ulaşır, argv üzerinden asla —
  argv, `/proc/<pid>/cmdline` ile okunabilir.
- Ham değerin yanı sıra base64, URL kodlanmış ve ters bölü kaçışlı yazımlarını
  da maskeler.
- Değeri **iki ayrı yazma arasına bölünmüş** hâliyle de yakalar; böylece bir
  flush sınırına denk gelen sır sızmaz.
- Tamamlanmış satırları anında geçirir, böylece geliştirme sunucusunun çıktısı
  sen izlerken tamponda beklemez.
- 6 karakterden kısa değerleri maskelemeyi reddeder ve bunu söyler: `5432`
  maskelenirse çıktıdaki ilgisiz her sayı bozulur.
- Çıkış kodu, stdin ve Ctrl-C çocuğa olduğu gibi ulaşır.

Yapay zekâ ajanı oturumunda ve stdout bir terminal olmadığı her durumda
öntanımlı olarak açık; kendi etkileşimli terminalinde renkler korunsun diye
kapalı. `--raw` zorla kapatır, `--mask` zorla açar.

### Ajan freni

```bash
maskrun install-guard        # Claude Code için bir PreToolUse hook'u kaydeder
```

Bir hook'un bütün anlamı, onu **modelin değil harness'ın uygulamasıdır**.
Prompt'a yazılan bir kuralı model uygular, dolayısıyla model kendini o kuraldan
konuşarak çıkarabilir. Bu, konuşularak aşılamaz.

Değeri transcript'e koyacak komutları reddeder:

- `maskrun get`, `maskrun import`
- `secret-tool lookup/search`, `security find-generic-password`
- `--raw`, `MASKRUN_MASK=0`, `MASKRUN_ALLOW_READ=1` (maskelemeyi kapatmak)
- `/proc/<pid>/environ`
- çıplak `env` / `printenv`, `Get-ChildItem Env:`
- bir `.env` / `.envrc` dosyasını okumak — Bash ile de olsa, ajanın dosya
  araçlarıyla da olsa

Ve normal işe karışmaz: `maskrun run`, `env VAR=x cmd`, `cat .env.example`,
`cat .maskrun`, `printenv PATH`, `ls -la .env`, `rm .env`.

`maskrun install-guard` mevcut `settings.json`'ına birleştirir, önce yedeğini
alır, idempotenttir, geçerli JSON olmayan bir dosyaya dokunmayı reddeder ve
`--remove` yaptığını geri alır. Diğer harness'lar: `maskrun hook`'u, araç
çağrısını stdin'den JSON olarak alan bir ön-araç hook'u olarak çalıştır — bkz.
[integrations/claude-code](integrations/claude-code/).

CLI aynı reddetmeleri kendisi de uygular; böylece hook desteği olmayan bir
harness'ta koşan bir ajan yine de `maskrun get` yapamaz.

### Hook mekanizması olmayan bir ajanı yönlendirmek

```bash
maskrun install-rules        # AGENTS.md/CLAUDE.md/.cursor kurallarına kısa bir blok yazar
```

`install-guard` yalnızca harness'ın senin adına bir PreToolUse hook'u
çalıştırdığı yerlerde işe yarar. Başka yerlerde hiçbir şeyi uygulatan bir
mekanizma yok — bu yüzden `install-rules`, projede zaten var olan `AGENTS.md`,
`CLAUDE.md` veya `.cursor/rules/` dosyalarından hangisi varsa ona (hiçbiri
yoksa `AGENTS.md` oluşturarak) işaretlenmiş kısa bir blok yazar ve ajana
maskrun'ı burada nasıl kullanacağını anlatır. **Bu yönlendirmedir,
uygulatma değildir**: model bunu, başka herhangi bir talimatı yok sayabildiği
gibi yok sayabilir. `install-guard`'ın ulaşamadığı harness'lar için bir geri
düşüş yoludur, onun yerine geçen bir şey değil.

Blok `<!-- maskrun:start -->`/`<!-- maskrun:end -->` işaretleriyle sınırlanır,
dosyanın önce yedeğini alır, idempotenttir (ikinci çalıştırma çoğaltmak yerine
yerinde günceller) ve `--remove`, dosyanın geri kalanına dokunmadan bloğu geri
çıkarır. Ortamda bir `.maskrun` manifest'i varsa blok, projenin beklediği
gerçek değişken adlarını listeler; `--file <path>` keşfi atlayıp doğrudan tek
bir dosyayı hedefler.

<a id="interactive-view"></a>

### Etkileşimli görünüm

```bash
maskrun            # kendi terminalinde, argümansız
```

Komut satırında başka hiçbir şey olmayan gerçek bir terminal, ok tuşlarıyla
gezilen bir görünüm açar: solda platformlar, sağda o platformun sırları ve
seçili olanın ayrıntısı (platform, not, değer).

```
↑↓ move   → enter   ← back   e edit   d delete   v reveal   y copy   q quit
```

Değerler sen `v`'ye basana kadar maskelidir (`••••••••••••••••`) ve bir değer
yalnızca o anda anahtarlıktan okunur — listede gezinmek ona hiç dokunmaz.
Başka bir sırra geçmek otomatik olarak yeniden maskeler. `e`, platformu, notu
ve değeri yerinde düzenler (boş bırakmak mevcudu korur); `d`, adıyla onay ister
(`delete 'name'? [y/N]`) ve yalnızca harfi harfine bir `y`/`Y` devam ettirir —
Enter dahil diğer her tuş iptal eder. `y`, değeri panona kopyalar.

Bir [alternatif ekran
tamponunda](https://en.wikipedia.org/wiki/Terminal_emulator#Alternate_screen_buffer)
çalışır: çizdiği hiçbir şey — açığa çıkarılmış bir değer dahil — terminalinin
geçmişine düşmez; bugün `maskrun get`'in çıktısının aksine. Bu, aşağıdaki her
şeyden bağımsız olarak gerçek bir iyileştirmedir.

Değere dokunan diğer her yol gibi, **bir yapay zekâ ajanı oturumunda asla
açılmaz** — çıktı borulanmış olsun olmasın, tespit edilmiş bir ajan oturumu her
zaman çıplak `maskrun`'ın etkileşimsiz bastığı aynı yalnızca-adlar özetini alır.
Ayrıca 60x15'ten küçük bir terminalde açılmayı reddeder ve o özete düşmeden önce
nedenini söyler.

#### Panoya kopyalama

`y`, seçili sırrı panona kopyalar; çünkü sonra hiçbir yere yapıştıramayacağın
bir değeri açığa çıkarmak sorunu sadece öteler — onu ya yeniden yazardın ya da
fareyle seçerdin, ikisi de daha kötü. Yerleşik üç sınır:

- **45 saniye sonra kendiliğinden temizlenir.** Bir şey kopyalandığında TUI
  canlı bir geri sayım gösterir; `c` beklemek yerine anında temizler ve TUI'den
  kopyalanmış bir şey dururken çıkmak da temizler. Temizleme, kopyadan önce
  panoda ne varsa onu geri koyar; hiçbir şey yoksa panoyu boşaltır.
- **Her platformda bir "kaydetme" ipucu gönderilir**, arboard'ın
  `exclude_from_history` özelliğiyle: Linux'ta KDE'nin
  `x-kde-passwordManagerHint` mime türü, macOS'ta topluluğun
  `org.nspasteboard.ConcealedType` uzlaşımı ve Windows'ta yerel
  `CanIncludeInClipboardHistory` pano biçimi. Burada gerçek bir Klipper'a karşı
  doğrulandı: düz bir kopya geçmişinde görünüyor, etiketlenmiş olanı
  görünmüyor ve Klipper etiketlenmiş olanı *güncel* pano içeriği olarak bile
  bildirmiyor. Bunların her biri, belirli bir aracın uymayı seçtiği bir
  ipucudur; maskrun'ın dayattığı bir şey değil — aşağıya bakın.
- **Bir ajan oturumunda asla açılmaz**, TUI'nin geri kalanıyla aynı kapı.

**Bunun çözmediği:** o ipucuna bakmayan herhangi bir pano geçmişi aracı
(GNOME'unki, KDE dışı Wayland kurulumlarının çoğu ve — bunu inşa ederken
doğrudan gözlemlendi — kompozitörünün kullandığı pano taşımasını izlemediğinde
KDE'nin kendi Klipper'ı bile) değeri kalıcı olarak kaydetmeye devam eder ve 45
saniyelik otomatik temizleme, bir geçmiş dosyasına girmiş o kopyaya hiçbir şey
yapmaz. Panoya kopyalamaya, aşağıdaki `/proc/<pid>/environ` ve maskelenmemiş
bir değeri elle yapıştıran bir insanla aynı gözle bak: çözülmüş bir sorun
değil, gerçek bir sınır.

## Bu ne değildir

**maskrun bir güvenlik sınırı değildir.** Kazalara karşı bir sertleştirmedir ve
sana — ya da senin tarafından başkasına — bundan fazlası olarak satılmamalıdır.

- **Anahtarlık depolamayı çözer, erişimi değil.** Senin kullanıcınla çalışan her
  süreç `secret-tool lookup` veya `security find-generic-password`
  çağırabilir; ajanın dahil. Fren, bunu kazayla yapmanın maliyetini yükseltir;
  imkânsız kılmaz. Aynısı senin için de geçerli: bir insan maskelenmemiş bir
  değeri transcript'e elle yapıştırırsa, o tuş vuruşundan sonrası için hiçbir
  araç onu yakalayamaz.
- **Frenin okuyamadığı kod.** Heredoc gövdeleri ve betik dosyaları komut değil
  veri olarak ele alınır — bilerek, çünkü onları ayrıştırmak yanlış pozitifler
  üretti. Yani `.env`'i kendi kaynağının içinden okuyan bir betik geçer. Bir
  desen eşleştirici keyfî kodu asla yakalayamaz.
- **Süreç ortamı.** `maskrun run` çalışırken, çocuğunun ortamı
  `/proc/<pid>/environ` ile okunabilir. Fren bu yolu doğrudan engeller ama ortam
  enjeksiyonu tasarımı gereği bu şekle sahiptir.
- **Panoya kopyalamayı otomatik temizleme geri almaz.** 45 saniyelik zaman
  aşımı *panoyu* boşaltır; o zaman aşımı dolmadan önce değeri kalıcı olarak
  kaydetmiş bir pano geçmişi aracına hiçbir şey yapmaz. maskrun kopyayı
  KDE'nin Klipper'ı atlasın diye etiketliyor — bu gerçek ve doğrulanmış, ama
  geçmiş tutan her araç o etikete uymuyor ve GNOME'un pano geçmişinin, KDE dışı
  Wayland kurulumlarının çoğunun ve Windows Pano Geçmişi'nin uyduğu bilinmiyor.
  Yukarıdaki `/proc/<pid>/environ` ve aşağıdaki, maskelenmemiş bir değeri elle
  yapıştıran insanla aynı rafta: sessizce çözülmüş değil, açıkça söylenmiş bir
  sınır.
- **Yazma sırasında `ps` — kapatıldı.** macOS'ta değer, `security`'ye bir CLI
  argümanı olarak ulaşıyordu ve `ps` üzerinden süreç listesinde kısa süre
  görünüyordu. O yol kapandı: maskrun artık doğrudan Security.framework'ün
  generic-password API'sini çağırıyor, yani değer, işletim sisteminin herkese
  göstermek zorunda olduğu bir argv hiç olmuyor.
- **Kabuk ayrıştırıcısı değil, boru hattı segmenti eşleştirmesi.** Fren, bir
  boru hattının her segmentini tek başına değerlendirir; `sed 's/maskrun get/x/'
  notes.md` komutunun geçmesini sağlayan da budur — o metin `maskrun get`'i hiç
  çalıştırmaz, sadece anar. Aynı kapsam, frenin bir komutu dolaylılık üzerinden
  takip etmediği anlamına gelir: `echo 'maskrun get x' | sh`, `sh`'nin sonunda
  çalıştıracağı komut olarak değil, bir `echo` olarak okunur.

Gerçek bir sınır istiyorsan, ajanın kabuğu anahtarlığa hiç erişemeyen bir yerde
çalışmalı — Linux'ta D-Bus oturum soketi olmadan, ayrı bir kullanıcı hesabında
ya da bir konteynerde. O zaman maskrun senin terminalinden çalışır, ajanınkinden
değil. Etki alanı sağlayıcı tarafında daha iyi yönetilir: araç başına ayrı
anahtar, harcama üst sınırları, rotasyon ve mümkün olan yerlerde kısa ömürlü
kimlik bilgileri.

## Arka uçlar

| Platform | Arka uç adı | Depolama |
|---|---|---|
| Linux | `secret-service` | libsecret'in Secret Service'i, doğrudan D-Bus üzerinden (`dbus-secret-service` crate'i) — gnome-keyring, KWallet, KeePassXC |
| macOS | `keychain` | login keychain, Security.framework'ün generic-password API'si üzerinden (`security-framework` crate'i) |
| Windows | `dpapi` | Windows Credential Manager (`windows` crate'i, `Win32_Security_Credentials`) — ad, override uyumluluğu için eski DPAPI-dosya arka ucundan korundu; altındaki depolama artık DPAPI dosyaları değil |

Önceki sürümler her işlem için Linux'ta `secret-tool`'a, macOS'ta `security`'ye
alt süreç olarak çıkıyor, Windows'ta ise DPAPI dosya depolamasını elle
yazıyordu. Üçü de artık `put`/`get`/`delete` için bir alt süreç ya da elle
yazılmış kripto yerine platform API'sine bağlanan bir kütüphaneden geçiyor —
Linux'ta `secret-tool` kurulumu gerekmiyor ve macOS'ta sır değeri artık bir CLI
argümanı olmuyor. (macOS'ta `list` adları saymak için hâlâ
`security dump-keychain`'e çıkıyor — o çağrı argüman olarak hiçbir sır değeri
almıyor, yani yeni bir şey açığa çıkmıyor; `security-framework`'te bunun yerine
konacak, servise göre kapsamlanmış bir sayım çağrısı yok.)

Linux depolaması, `secret-tool lookup service maskrun name <name>` komutunun
beklediği şemayı (`service` + `name` öznitelikleri) koruyor; yani maskrun'ın
sakladığı bir sır, elle kontrol etmen gerekirse standart CLI ile hâlâ
okunabilir — maskrun'ın kendisi sadece artık o CLI'nin kurulu olmasına bağımlı
değil.

### Platform etiketi ve not nerede duruyor

| Platform | Nerede |
|---|---|
| Linux | Mevcut `service`/`name`'in yanında iki ek Secret Service özniteliği, `platform` ve `note` — `secret-tool lookup` yalnızca verdiğin özniteliklerle eşleşir, dolayısıyla bunlar `service`+`name` okuyan hiçbir şeyi etkilemeden yanlarında taşınır. |
| macOS | Generic password'ün `kSecAttrDescription` (platform) ve `kSecAttrComment` (not) alanları; `security-framework`'ün öznitelik arama ve yalnızca-öznitelik güncelleme API'leriyle yazılıp okunuyor — yeniden etiketleme saklanan değerin kendisine hiç dokunmuyor. |
| Windows | İkisi de Credential Manager'ın sunduğu tek `CREDENTIALW.Comment` alanına kodlanıyor (`platform=<p><US>note=<n>`, `<US>` = U+001F): burada ayrı bir öznitelik deposu yok ve `CredWriteW`'nin kısmi güncelleme çağrısı olmadığı için yeniden etiketleme kaydın tamamını (değer dahil) yeniden yazmak zorunda. |

Tespiti `MASKRUN_BACKEND=secret-service|keychain|dpapi` ile geçersiz kıl.

### Gerçekte doğrulanmış olan

Üç arka uç da CI'da gerçek bir anahtarlığa karşı çalıştırılıyor: Linux'ta
Secret Service, macOS'ta Keychain, Windows'ta Credential Manager. Anahtarlık
testleri arka ucu taklit etmek yerine gerçek bir sırrı gidip geri getiriyor ve
hiç anahtarlık kurulu olmayan bir iş, frenin yine de cevap verdiğini
kanıtlıyor.

Platform etiketi / not özelliği için de hikâye aynı: Linux'ta gerçek bir Secret
Service'e karşı çalıştırıldı (`secret-tool`/öznitelik şeması uyumluluk kontrolü
dahil), macOS ve Windows için platform kodunun geri kalanıyla aynı şekilde
çapraz hedefte tip denetiminden geçti, ama gerçek bir Keychain veya Credential
Manager üzerinde ancak o CI işleri koştuğunda gerçekten çalıştırılmış olacak.

Başlayan ilk CI koşusu, platform kodunda üç gerçek hata buldu; hepsi, `cfg` ile
kapılandıkları için bir Linux derlemesinin hiç tip denetimi yapmadığı
yollardaydı: yanlış tipte iki Win32 argümanı, hem bir filtre hem de
tüm-kimlik-bilgileri bayrağını birlikte geçiren (ikisi birlikte geçersiz) bir
`CredEnumerateW` çağrısı ve `ERROR_NOT_FOUND`'un, gerçekte gelen `HRESULT`
yerine ham Win32 koduyla karşılaştırılması — bu, eksik bir sırla boş bir deponun
ikisinin de ham hata olarak görünmesine yol açıyordu. Lint işi artık Windows ve
macOS hedeflerini Linux'tan çapraz denetliyor, böylece bu sınıf hata bir daha
bir platform runner'ına ulaşamaz.

Kapsam *dışında* olan: kilit yönetimi testleri opt-in
(`MASKRUN_LOCK_TESTS=1`), çünkü gerçek bir masaüstünde tek kullanımlık bir
`gnome-keyring-daemon` başlatmak kullanıcıdan anahtarlık oluşturmasını istiyor
ve testten sonra da yaşamaya devam ediyor.

## Komutlar

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

`import`, `VITE_`, `NEXT_PUBLIC_`, `PUBLIC_`, `REACT_APP_`, `NUXT_PUBLIC_`,
`EXPO_PUBLIC_` ve `GATSBY_` değişkenlerini atlar: bunlar istemci paketine
derlenip her ziyaretçiye gönderilir, yani sır değil yapılandırmadırlar ve
taşınmaları hiçbir şey kazandırmaz. `--all` bunu geçersiz kılar.

### Platform etiketleri ve notlar

Birkaç sırrı geçtikten sonra, her birinin ne işe yaradığını hatırlamak için düz
bir ad yetmez — özellikle bir platformun birden fazla anahtarı varsa ve
aralarındaki fark ad değil kapsamsa. `--for` bir sırrı bir platform/servisle
etiketler; `--note` kısa bir serbest metin açıklama ekler:

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

Her iki bayrak da isteğe bağlıdır ve hâlihazırda ikisi de olmayan sırlara
uygulanır. `label`, mevcut bir sırrı sonradan yeniden etiketler; her iki
komutta da `--for ""` (veya `--note ""`) vermek o alanı temizler, bir bayrağı
tamamen yazmamak ise mevcut değere dokunmaz — bir sırrın değerinin üstüne `put`
yapmak etiketini asla sessizce düşürmez. **Platform ve not sır değildir**:
maskelenmeden saklanırlar, bir yapay zekâ ajanı oturumunda görünürler ve `list`
ile `status` tarafından gösterilirler. `--note` içine bir sır değeri koyma; 200
karakterle sınırlıdır ve satır sonu içeremez.

Argümansız `maskrun`, gerçek bir terminalde [etkileşimli
görünümü](#interactive-view) açar; borulandığında (`maskrun | cat`),
etkileşimsizken veya bir ajan oturumunda ise bunun yerine kısa bir özet basar:
manifest durumu ve yukarıdaki gruplanmış sır listesi — böylece komut yüzeyini
aklında tutmak zorunda kalmazsın.

### Kabuk tamamlaması

```bash
maskrun completions fish > ~/.config/fish/completions/maskrun.fish
maskrun completions bash > ~/.local/share/bash-completion/completions/maskrun
maskrun completions zsh > ~/.zfunc/_maskrun   # sonra compinit'ten önce `fpath+=~/.zfunc`
```

Bayrakların ve alt komutların ötesinde, `maskrun get`/`rm`/`label` gerçek sır
adlarını tamamlar — fish bunu kutudan çıktığı gibi yapar (bu bayrağın var olma
nedeni olan aynı `maskrun list --plain` üzerinden); bash/zsh yalnızca statik
tamamlamaları alır.

## Ortam değişkenleri

| Değişken | Etkisi |
|---|---|
| `MASKRUN_MASK` | `1` her zaman maskele, `0` asla maskeleme |
| `MASKRUN_AGENT` | `1` bunu bir ajan oturumu say, `0` insan say |
| `MASKRUN_BACKEND` | bir arka ucu zorla |
| `MASKRUN_ALLOW_READ` | `1` bir ajan oturumunda `get`/`import`'u yeniden açar |

Ajan oturumları `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `AI_AGENT`,
`AIDER_CHAT`, `CURSOR_AGENT`, `OPENAI_CODEX`, `GEMINI_CLI` ve `REPLIT_AGENT`
değişkenlerinden tespit edilir. Bunların dışındaki her şey için harness'ta
`MASKRUN_AGENT=1` ayarla.

## Platform desteği

- **Linux** — glibc 2.35 veya daha yenisi (release binary'leri Ubuntu 22.04
  üzerinde derleniyor). CI'da canlı bir Secret Service'e karşı test edildi.
- **macOS** — Intel ve Apple Silicon. CI'da gerçek bir Keychain'e karşı test
  edildi.
- **Windows** — x86_64. CI'da gerçek Credential Manager'a karşı test edildi.

## Önceki çalışmalar

[`envchain`](https://github.com/sorah/envchain) sırları yıllar önce keychain'e
koyup ortama enjekte etti ve bu aracın `run` yarısının doğrudan atasıdır.
[`direnv`](https://direnv.net/) dizin başına ortamları yönetir,
[`sops`](https://github.com/getsops/sops) ve
[`dotenvx`](https://github.com/dotenvx/dotenvx) sırları depoda şifreli olarak
saklar, [`aws-vault`](https://github.com/99designs/aws-vault) ise tek bir
sağlayıcı için keychain dansını yapar.

maskrun'ın eklediği, ajana bakan yarı: çocuğun çıktısını maskelemek ve bir
değeri sızdıracak komutlara harness'ın uyguladığı bir fren koymak. Bir yapay
zekâ ajanıyla çalışmıyorsan, `envchain` sana yetebilir.

## Geliştirme

```bash
cargo test -- --test-threads=1   # tek iş parçacığı: arka uçlar gerçek anahtarlık durumunu paylaşır
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make test                        # aynı test koşusu
make lint                        # fmt --check + clippy
```

Anahtarlık testleri, erişilebilir bir arka uç yoksa kendilerini atlar; böylece
çıplak bir konteynerde fren testleri yine de koşar.

Test takımındaki sır değerleri rastgele üretilir ve asla basılmaz — iddialar
yokluk ya da varlık kontrol eder, loglanmış bir değerle eşitlik asla.

Katkılar memnuniyetle karşılanır. Bir arka uç eklemek, `Backend` trait'inin
`put`/`get`/`delete`/`list` metotlarını uygulamak demektir; bir harness eklemek
ise `integrations/` altında bir girdi demektir.

## Lisans

Copyright (C) 2026 Furkan Akyol.

maskrun özgür yazılımdır: Free Software Foundation tarafından yayımlanan GNU
Genel Kamu Lisansı'nın 3. sürümü ya da daha sonraki herhangi bir sürümü altında
yeniden dağıtabilir ve değiştirebilirsin. Hiçbir garanti vermez. Tam koşullar
için bkz. [LICENSE](LICENSE).

Pratik bir sonucu: değiştirilmiş bir maskrun'ı dağıtıyorsan — kaynak olarak,
binary olarak ya da bir ürünün içinde — değiştirilmiş kaynağını, dağıttığın
kişiye aynı lisans altında sunmak zorundasın.
