# Preverjanje različice 0.1.0

Preverjeno 29. 10. 2026 na macOS / Apple Silicon, Rust 1.92.0.

## Uspešno

- Release build odjemalca in strežnika.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` brez opozoril.
- 4 unit testi: validacija podatkov, paketi, pretvorba vzorčne frekvence, Opus 128 kb/s in jitter buffer.
- Integracijski test z dvema dejanskima QUIC strežnikoma in več odjemalci: ločevanje sob in strežnikov, mute, ping kontrolnega toka, ping datagramov, identiteta pošiljatelja, napačno geslo, napačen pripeti certifikat, omejitev kapacitete in odjava.
- Ročni preizkus zgrajene `.app` prek uporabniškega vmesnika: povezava na lokalni strežnik, potrditev dejanskega odtisa, prikaz pinga, vstop v Klic 1, prikaz pinga klica in utišanje mikrofona z oznako ob imenu.
- Aplikacija se je povezala in vstopila v sobo brez napake zvočne naprave. Izmerjeni lokalni ping je bil manj kot nekaj ms; to ni meritev za internetno povezavo.
- macOS paket ima veljaven ad-hoc podpis; odjemalec ne potrebuje ločene knjižnice Opus.

## Ni neposredno preizkušeno

- Poslušanje pogovora med dvema fizičnima računalnikoma in dejanska kakovost zajema mikrofona.
- Windows, Linux in macOS Intel buildi ter delovanje na teh napravah. Priložen je workflow za gradnjo na vseh ciljnih sistemih.
- Namestitev systemd storitve na oddaljeni Linux strežnik.
- Internetni NAT/požarni zid, večje obremenitve, meritve porabe CPU/RAM, slabe omrežne razmere in Bluetooth naprave.

Za zvočni test naj se dva uporabnika s slušalkami povežeta v isto sobo. Preverita govor v obe smeri, mute, prehod v različni sobi in ponoven vstop. Če mikrofon ni dovoljen, omogoči dostop v sistemskih nastavitvah in aplikacijo ponovno zaženi.
