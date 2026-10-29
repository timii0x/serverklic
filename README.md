# ServerKlic

Majhna namizna aplikacija v Rustu za Windows, macOS in Linux ter lasten Linux strežnik. Štiri glasovne sobe, imena, mute, prikaz govorca in sproten ping. Brez Discord računa ali zunanje storitve.

## Najhitrejši test na tem Macu

V `dist/ServerKlic-macos-arm64/` dvoklikni **Zazeni-lokalni-test.command**. Ta zažene lokalni strežnik in aplikacijo.

1. Vpiši ime in `127.0.0.1:5000`; geslo pusti prazno.
2. Klikni **Poveži se**. Ob prvi povezavi primerjaj odtis s terminalom in klikni **Zaupam temu strežniku in se poveži**.
3. Dovoli dostop do mikrofona. Klikni **Pridruži se** pri Klic 1.
4. Za pogovor potrebuješ še drugega odjemalca v isti sobi. Uporabi slušalke. Za dodatno lokalno okno v terminalu uporabi `open -n /pot/do/ServerKlic.app` (brez ponovnega zagona strežnika).
5. ENTER v terminalu zaustavi lokalni testni strežnik. Zapiranje aplikacije prekine njen klic.

Ta pripravljeni paket je za **Apple Silicon (arm64)**. Projekt vsebuje izvorno kodo in avtomatsko gradnjo za Windows x64, Linux x64 ter macOS Intel in Apple Silicon. Windows/Linux/Intel paketi v tej seji niso bili zgrajeni ali preizkušeni na teh sistemih; ustvari jih spodnji build oziroma GitHub Actions.

Lokalni testni strežnik posluša samo na `127.0.0.1`, zato ni dostopen z drugega računalnika. Za LAN test zaženi strežnik z `--bind 0.0.0.0:5000` in na drugem računalniku vpiši LAN IP gostitelja, npr. `192.168.1.20:5000`. Za internet uporabi javni IP strežnika ter odpri **UDP** port. Naslov IPv6 zapisuj kot `[::1]:5000`; za IPv6 naj strežnik posluša na `[::]:5000`.

## Kaj prikazujeta pinga

- **Strežnik:** aplikacijski povratni čas zahteve in odgovora po zanesljivem QUIC toku, tudi v preddverju.
- **Klic:** povratni čas majhnega datagrama po isti QUIC datagramski poti kot zvok, samo med članstvom v sobi. Deluje tudi pri utišanem mikrofonu.
- Oba se merita vsako sekundo. Podatek, starejši od 3 sekund, se spremeni v »ni odziva«. Izven sobe je ping klica »—«.
- Ping ni celotna zakasnitev mikrofona do slušalk drugega uporabnika. K tej se prištejejo zajem, Opus, približno 60 ms predpomnilnika za nihanje omrežja in predvajanje.

## Linux strežnik: namesti in zaženi

Na Debian/Ubuntu s systemd prenesi projekt, odpri terminal v mapi projekta in izvedi:

```bash
chmod +x serverklic.sh
./serverklic.sh install
./serverklic.sh start --port 5000
./serverklic.sh logs --port 5000
```

`install` namesti potrebna orodja za prevajanje, po potrebi Rust 1.92.0, prevede samo strežnik, namesti binarno datoteko in systemd predlogo. Za sistemsko namestitev uporablja sudo. Strežnik nato teče kot namenski uporabnik `serverklic`, se ob sesutju ponovno zažene in je omogočen ob zagonu računalnika.

Če uporabljaš UFW, odpri port (skripta ne spreminja pravil požarnega zidu):

```bash
sudo ufw allow 5000/udp
```

Za več ločenih instanc:

```bash
./serverklic.sh start --port 5001
./serverklic.sh start --port 5002
./serverklic.sh status --port 5001
./serverklic.sh restart --port 5001
./serverklic.sh stop --port 5001
```

Odpri tudi UDP 5001/5002, če ju uporabljaš. Vsaka instanca ima svoje štiri sobe in certifikat v `/var/lib/serverklic/PORT/`. `stop` instanco ustavi in onemogoči njen samodejni zagon. Po posodobitvi z `install` ponovno zaženi uporabljene instance.

Neobvezno geslo nastavi v `/etc/serverklic/5000.env`:

```text
SERVERKLIC_PASSWORD="tvoje-dolgo-geslo"
```

Datoteko naj lahko bere samo root (`sudo chmod 600 /etc/serverklic/5000.env`), nato `./serverklic.sh restart --port 5000`. Geslo v aplikaciji ni shranjeno na disk. Brez gesla se lahko poveže vsak, ki ima naslov in doseže port. Imena niso registrirani računi; lahko se ponavljajo.

Brez systemd lahko prevedeni strežnik zaženeš neposredno:

```bash
./serverklic-server --bind 0.0.0.0:5000 --data-dir ./server-data/5000
```

Strežnik privzeto dopušča največ 32 uporabnikov skupaj v vseh sobah. Omejitev lahko znižaš z `--max-clients 12`. Zapreš ga s Ctrl+C.

## Gradnja namizne aplikacije

Uporabnik končnega paketa ne potrebuje Rust orodij. Za izdelavo paketov potrebuješ Rust 1.92.0 ali novejši, C prevajalnik, CMake in Python 3. Opus se prevede in statično vključi; strežnik kodeka sploh ne potrebuje.

### macOS

Namesti Xcode Command Line Tools (`xcode-select --install`), Rust z rustup.rs ter CMake (npr. `brew install cmake`), nato:

```bash
./scripts/build.sh
```

Nastaneta `.app` in ZIP v `dist/`. Aplikacija ima opis dovoljenja za mikrofon in lokalni ad-hoc podpis. Za javno distribucijo brez opozoril Gatekeeper potrebuješ lasten Apple Developer podpis in notarizacijo. Za lokalni test uporabi desni klik → Open, če macOS pokaže opozorilo. Dovoljenja preveri v System Settings → Privacy & Security → Microphone. Privzeti mikrofon in izhod izbereš v sistemskih nastavitvah pred povezavo; po menjavi naprav aplikacijo ponovno poveži.

### Windows x64

Namesti Rust (MSVC), Visual Studio Build Tools z »Desktop development with C++«, CMake in Python 3. V razvojnem PowerShell terminalu:

```powershell
.\scripts\build.ps1
```

V `dist/ServerKlic-windows-amd64/` oziroma mapi z ustrezno oznako arhitekture dobiš `serverklic.exe`, strežnik in `Zazeni-lokalni-test.bat`. Končni uporabnik samo razširi ZIP in odpre EXE. Paket nima javnega Authenticode podpisa.

### Linux namizje (Debian/Ubuntu)

```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake pkg-config python3 libasound2-dev libx11-dev libxi-dev libxcursor-dev libxrandr-dev libxinerama-dev libgl1-mesa-dev libwayland-dev libxkbcommon-dev
./scripts/build.sh
```

Namesti tudi Rust, če ga še nimaš. Paket v `dist/` vsebuje odjemalca, strežnik in `zazeni-lokalni-test.sh`. Linux binarna aplikacija potrebuje sistemske knjižnice ALSA, grafičnega okolja in OpenGL; ni univerzalni AppImage. Paket iz Ubuntu 22.04 je namenjen združljivim distribucijam.

### Avtomatski paketi za vse sisteme

Projekt naloži v svoj GitHub repozitorij. Pod **Actions → Build desktop apps → Run workflow** zaženi priloženi workflow. Za vsak OS se izvedejo testi, release build in pakiranje ZIP. Pakete preneseš pod Artifacts. Ta projekt sam ne ustvarja repozitorija in ničesar ne objavlja.

## Preverjanje

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Integracijski test zažene dve začasni instanci na naključnih lokalnih portih. Preveri povezovanje, ločevanje sob in instanc, popravljanje pošiljateljevega ID-ja, mute, izhod, oba pinga, geslo, zavrnitev spremenjenega certifikata, omejitev uporabnikov in čiščenje po prekinitvi. Testi zvoka preverijo Opus paket pri 128 kb/s, preurejanje paketov, nadomeščanje izgube in trajanje pretvorbe frekvence. Testi ne nadomestijo poslušanja z dvema fizičnima napravama.

## Tehnične podrobnosti in meje prve različice

- Native Rust GUI: egui/eframe, brez Electron ali brskalnika.
- QUIC/TLS 1.3 z rustls; zanesljivi tokovi za stanje in datagrami za zvok. Certifikat strežnika je trajen in pripet po ročni potrditvi prve povezave. Če se certifikat spremeni, aplikacija povezavo zavrne. Po legitimni menjavi certifikata odstrani stari zapis `pins` iz nastavitev aplikacije ali uporabi drug naslov/port in preveri novi odtis.
- Šifriranje velja med aplikacijo in strežnikom, ni end-to-end med uporabniki; upravljavec strežnika lahko dostopa do zvočnih paketov.
- Opus mono, 48 kHz, 20 ms na paket, CBR 128.000 bit/s (320 bajtov na zvočni paket, brez omrežnih glav). Zajeta frekvenca se po potrebi pretvori. Uporabljeni so privzeti zvočni napravi in formati f32/i16/u16.
- Strežnik samo posreduje zvok. Pri N aktivnih govorcih v eni sobi je izhod približno N × (N−1) × 128 kb/s + protokol. Pri 10 sočasno oddajajočih je to približno 11,52 Mb/s. Ta različica oddaja tudi tišino, dokler je mikrofon v sobi vključen.
- Približno 60 ms začetnega jitter bufferja, omejene vrste, Opus concealment pri manjkajočih paketih; brez prilagodljivega profesionalnega jitter bufferja.
- Ni odstranjevanja odmeva, zatiranja šuma, avtomatskega ojačanja, izbire naprav v aplikaciji, push-to-talk, samodejne ponovne povezave, mobilne verzije ali moderacije uporabnikov. Za klic uporabi slušalke.
- CPU/RAM številke niso obljubljene: odvisne so od naprav, števila govorcev, OS in grafičnega gonilnika. Strežnik zvoka ne dekodira.

Izvorna koda: `crates/protocol`, `crates/server`, `crates/client`. `Cargo.lock` zaklepa preverjene odvisnosti.
