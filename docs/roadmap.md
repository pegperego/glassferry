# Roadmap

Sviluppi futuri di SSH File Manager. Ogni proposta ha la data in cui è nata e il motivo; quando viene realizzata passa in fondo, in "Fatto", con il commit.

Stato: **da fare** (deciso, va pianificato), **proposta** (da valutare), **in analisi**, **in corso**.

## Prossimi passi

| Voce | Stato | Proposta il | Note |
|---|---|---|---|
| Prova con configurazioni ssh complesse | proposta | 2026-10-27 | ProxyJump, 1Password (`IdentityAgent`), 2FA/TOTP, `Match`: non presenti nel config usato per la prova sul campo |
| Pubblicazione su GitHub | da fare | 2026-10-27 | Deciso all'inizio: git locale ora, GitHub dopo l'MVP |
| Build firmata e notarizzata per macOS (arm64 e x64) | da fare | 2026-10-27 | Distribuzione fuori dal Mac App Store: la sandbox impedirebbe di avviare `ssh` |
| Pacchetti Linux (AppImage / deb) | proposta | 2026-10-27 | Linux è una piattaforma supportata, ma finora provato solo su macOS |

## Funzioni

| Voce | Stato | Proposta il | Note |
|---|---|---|---|
| Ripresa dei trasferimenti interrotti | proposta | 2026-10-27 | SFTP permette di riprendere da un offset; candidata per la v1.1 |
| chmod / chown da interfaccia | proposta | 2026-10-27 | Anche ricorsivo con `chmod -R` quando la shell è disponibile |
| Dimensione di una cartella | proposta | 2026-10-27 | `du -sk` via shell, visita SFTP altrimenti |
| Ricerca di file sul server | proposta | 2026-10-27 | `find` via shell |
| Comprimere ed estrarre archivi sul server | proposta | 2026-10-27 | `tar` via shell; non possibile sugli account solo SFTP |
| Diff nel conflitto di modifica | proposta | 2026-10-27 | Oggi il dialogo mostra solo dimensione e data |
| Scegliere l'app di modifica per tipo di file | proposta | 2026-10-27 | Oggi si usa sempre l'app predefinita del sistema |
| Segnalibri per host e cartella | proposta | 2026-10-27 | |
| Gestione connessioni: elenco interno (importate da `~/.ssh/config` o configurate nell'app), categorie con colore, password nel Portachiavi, toolbar con ⌘K e modale di gestione | fatto | 2026-10-27 | [`connections.md`](connections.md), fase 1 (`0bc1986`) |
| Fino a 2 connessioni in pannelli affiancati o impilati, drag & drop tra tutti i pannelli | in corso | 2026-10-27 | [`connections.md`](connections.md), fase 2: implementata (anche i divisori ridimensionabili), da provare nell'app; il trascinamento remoto → remoto arriva con la fase 4 |
| Conferme rinforzate per le categorie protette, con anteprima dei comandi | da fare | 2026-10-27 | [`connections.md`](connections.md), fase 3 |
| Trasferimenti da remoto a remoto (streaming via Mac, `cp -a` sulla stessa connessione) | da fare | 2026-10-27 | [`connections.md`](connections.md), fase 4; assorbe "Copia lato server tra cartelle remote" |
| Impostazione del binario `ssh` | proposta | 2026-10-27 | Default `/usr/bin/ssh`; serve per chi usa l'ssh di Homebrew |
| Trascinare file dall'app al Finder | proposta | 2026-10-27 | Non supportato da Tauri: serve un plugin e, per i remoti, le file promise di macOS |
| Interfaccia in inglese | proposta | 2026-10-27 | Oggi i testi sono solo in italiano |

## Limiti noti da risolvere

| Voce | Stato | Proposta il | Note |
|---|---|---|---|
| La disconnessione chiude i file aperti anche se non sincronizzati | proposta | 2026-10-27 | Chiedere conferma o tenere la copia locale finché l'utente non decide |
| Conflitti di modifica senza `sha256sum` | proposta | 2026-10-27 | Sugli account solo SFTP una modifica altrui nello stesso secondo e con la stessa dimensione non viene vista (mtime con risoluzione di 1 s in SFTP v3) |
| Scrittura in place del file modificato | proposta | 2026-10-27 | Mantiene proprietario e permessi, ma se la connessione cade a metà il file resta parziale; valutare una modalità opzionale temporaneo + rename |

## Fuori scope

Scelte esplicite, da riconsiderare solo con una motivazione nuova.

| Voce | Motivo |
|---|---|
| Windows | Il codice usa ControlMaster e socket Unix; l'architettura lascia la porta aperta (un processo `ssh -s` per sessione, senza multiplexing) |
| Integrazione File Provider con il Finder | Fuori dagli obiettivi del progetto |
| macFUSE o altre dipendenze closed source | Requisito del progetto |
| Installare qualcosa sui server | Requisito del progetto: si usano solo il sottosistema SFTP e, se c'è, la shell |

## Fatto

| Voce | Commit |
|---|---|
| Connessione tramite il binario `ssh` di sistema, askpass integrato | `f29aa3f` |
| Doppio pannello locale / remoto | `4fd7499` |
| Pannello di log con file di log e modalità diagnostica | `a41264c` |
| Trasferimenti con coda, conflitti e drag & drop | `263c3c5` |
| Operazioni sui file: nuova cartella, rinomina, sposta, elimina | `737d0ce` |
| Modifica dei file remoti con l'app predefinita | `c89eb90` |
| Gestione connessioni (fase 1 di `connections.md`) | `0bc1986` |
| Prova sul campo con i server reali: esito positivo (2026-10-27) | — |
