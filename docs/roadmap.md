# Roadmap

Sviluppi futuri di Glassferry. Ogni proposta ha la data in cui è nata e il motivo; quando viene realizzata passa in fondo, in "Fatto", con il commit.

Stato: **da fare** (deciso, va pianificato), **proposta** (da valutare), **in analisi**, **in corso**.

## Prossimi passi

| Voce | Stato | Proposta il | Note |
|---|---|---|---|
| Prova con configurazioni ssh complesse | proposta | 2026-10-27 | ProxyJump, 1Password (`IdentityAgent`), 2FA/TOTP, `Match`: non presenti nel config usato per la prova sul campo |
| Build notarizzata per macOS | da fare | 2026-10-27 | Distribuzione fuori dal Mac App Store: la sandbox impedirebbe di avviare `ssh`. Manca l'account Apple Developer (99 $/anno): intanto le build sono firmate ad-hoc; poi basta aggiungere i secret a `release.yml` |
| Pacchetti Linux (AppImage / deb / rpm) | in corso | 2026-10-27 | Prodotti da `release.yml`; da provare su una macchina Linux vera, finora l'app è stata usata solo su macOS |

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
| Gestione connessioni: elenco interno (importate da `~/.ssh/config` o configurate nell'app), categorie con colore, password nel Portachiavi, toolbar con ⌘K e modale di gestione | fatto | 2026-10-27 | [`connections.md`](connections.md), fase 1 (`6020ada`) |
| Fino a 2 connessioni in pannelli affiancati o impilati, drag & drop tra tutti i pannelli | fatto | 2026-10-27 | [`connections.md`](connections.md), fase 2 (`d3decbb`, drag & drop nella finestra nativa `05d4b66`) |
| Conferme rinforzate per le categorie protette, con anteprima dei comandi | fatto | 2026-10-27 | [`connections.md`](connections.md), fase 3 (`01e0184`) |
| Trasferimenti da remoto a remoto (`copy-data`/`cp -p` sulla stessa connessione, streaming via Mac tra server diversi) | fatto | 2026-10-27 | [`connections.md`](connections.md), fase 4 (`21b37ec`); assorbe "Copia lato server tra cartelle remote" |
| Impostazione del binario `ssh` | proposta | 2026-10-27 | Default `/usr/bin/ssh`; serve per chi usa l'ssh di Homebrew |
| Trascinare file dall'app al Finder | proposta | 2026-10-27 | Non supportato da Tauri: serve un plugin e, per i remoti, le file promise di macOS |
| Interfaccia in inglese | proposta | 2026-10-27 | Oggi i testi sono solo in italiano |
| Aprire il terminale di sistema nella cartella del pannello (tasto rapido e menu contestuale) | da fare | 2026-10-28 | Per passare alla riga di comando senza riposizionarsi a mano. Pannello locale: il terminale si apre nella cartella corrente (`open -a Terminal <dir>`, `x-terminal-emulator`/`$TERMINAL` su Linux). Pannello remoto: nuova connessione `ssh -t` con lo stesso alias e `-F` della connessione, che fa `cd` nel percorso del pannello e avvia la shell di login; da valutare il riuso del ControlMaster (`-S <sock>`) per non richiedere di nuovo password o OTP. Non disponibile sugli account solo SFTP. Tasto ⌥⌘T (deciso 2026-10-28). App terminale configurabile nelle impostazioni (Terminal, iTerm2, Ghostty…; su Linux anche un comando libero), default Terminal su macOS |

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
| Connessione tramite il binario `ssh` di sistema, askpass integrato | `6b1152d` |
| Doppio pannello locale / remoto | `8824bec` |
| Pannello di log con file di log e modalità diagnostica | `01476f5` |
| Trasferimenti con coda, conflitti e drag & drop | `909f81e` |
| Operazioni sui file: nuova cartella, rinomina, sposta, elimina | `a0d12bb` |
| Modifica dei file remoti con l'app predefinita | `fbe91a1` |
| Gestione connessioni (fase 1 di `connections.md`) | `6020ada` |
| Fino a due pannelli remoti, affiancati o impilati (fase 2) | `d3decbb`, `05d4b66` |
| Operazioni con anteprima e conferme per le connessioni protette (fase 3) | `01e0184` |
| Copie da remoto a remoto (fase 4) | `21b37ec` |
| Pannelli a schede con il colore della categoria | `6a44ab9` |
| Nuovo nome Glassferry e nuova icona | `8884374` |
| Fasi 2–4 provate nell'app (2026-10-28) | — |
| Pubblicazione su [GitHub](https://github.com/pegperego/glassferry) (2026-10-28) | — |
| CI (test e clippy su macOS e Linux) e workflow di release con build macOS firmate ad-hoc | `ee45aaa` |
| Prova sul campo con i server reali: esito positivo (2026-10-27) | — |
