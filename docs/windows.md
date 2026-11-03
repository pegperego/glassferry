# Supporto per Windows

Analisi del 2026-11-03. Stato: proposte accettate, da cominciare con la prova di fattibilità (fase 0). Voce collegata in [`roadmap.md`](roadmap.md).

## Il problema: niente ControlMaster

Oggi una sessione è un **master** (`ssh -M`, autenticato una volta), **un canale SFTP** condiviso da liste, trasferimenti e modifiche, e un **exec** per ogni comando shell (probe, `rm -rf`, `du`, `find`, `tar`, `chmod`/`chown`, `sha256sum`, in futuro `tail -F`), tutti attraverso il master.

L'OpenSSH di Microsoft (Win32-OpenSSH, in `C:\Windows\System32\OpenSSH`) non supporta il multiplexing: la richiesta è aperta dal 2016 ([PowerShell/Win32-OpenSSH#405](https://github.com/PowerShell/Win32-OpenSSH/issues/405), "It's just not implemented"); l'unico tentativo è una pull request non accettata (openssh-portable #863), con un comportamento diverso da Unix. Ragioni tecniche probabili (nostra ricostruzione, non spiegate nella issue): il client del multiplexing passa al master i propri descrittori di file sul socket Unix (`SCM_RIGHTS`), che l'`AF_UNIX` di Windows non supporta; `ControlPersist` e `-f` usano `fork()`.

Senza master, ogni exec è una connessione nuova con autenticazione completa: con chiave o agent costa 200–500 ms, con password, OTP o 2FA chiederebbe le credenziali a ogni comando.

## Opzioni valutate

### Quale ssh

| # | Opzione | ControlMaster | Agent Windows / 1Password | Config come nel terminale | Esito |
|---|---|---|---|---|---|
| 1a | ssh di sistema Microsoft | no | sì | sì | **predefinito** (richiede ≥ 8.4) |
| 1b | ssh di Git for Windows / MSYS2 / Cygwin, scelto in Impostazioni | sì (da verificare) | no (cerca un socket Unix, non la named pipe `\\.\pipe\openssh-ssh-agent`) | percorsi Unix | **possibile**, scelta dell'utente |
| 1c | ssh dentro WSL (`wsl.exe ssh`) | sì | no (quelli di Linux) | config di Linux | **possibile**, più avanti |
| 1d | Win32-OpenSSH incluso nell'app | no | sì | sì | **ripiego facoltativo** se quello di sistema è < 8.4 |
| 1e | ssh Cygwin/MSYS2 incluso nell'app | sì | no | percorsi Unix | scartata: limiti di 1b più la sicurezza a carico nostro |
| 1f | fork di Win32-OpenSSH con la PR #863 | diverso da Unix | sì | sì | scartata: un fork di OpenSSH da mantenere |
| 1g | PuTTY / plink (`-share`) | a modo suo | Pageant | no (`~/.ssh/config` ignorato) | scartata: contro il principio del prodotto |
| 1h | libreria SSH nell'app (russh, libssh2) | nativo | da fare a mano | no (il config lo interpreteremmo noi) | scartata: contro il principio del prodotto |

Un ssh incluso nell'app (1d, 1e) sposta su di noi gli aggiornamenti di sicurezza (una vulnerabilità come regreSSHion vuol dire una release immediata) e crea due ssh diversi sulla stessa macchina, la situazione che il principio "si usa l'ssh di sistema" vuole evitare: per questo 1d è solo una scelta esplicita in Impostazioni, mai automatica.

### Come gestire la sessione

| # | Opzione | Esito |
|---|---|---|
| 2a | **Solo SFTP**: un processo `ssh -s sftp` per sessione, shell disattivata (`capabilities.shell = false`); tutto ripiega sui percorsi SFTP già esistenti | **prima fase** |
| 2b | **Una connessione per comando shell**, solo se l'autenticazione è non interattiva (prova con `BatchMode=yes`: chiave, agent, password salvata senza secondo fattore) | **seconda fase** |
| 2c | **Modalità scelta in base all'ssh**: master quando funziona (`ssh -V` che non riporta `OpenSSH_for_Windows_`, più una prova reale del master), altrimenti 2a/2b | **scelta**: copre 1a, 1b, 1c con lo stesso codice, vale anche su macOS/Linux |

In modalità solo SFTP si perdono gli archivi, chown/chgrp e la copia lato server con `cp -p` (resta `copy-data` se il server la supporta); ricerca, dimensioni, eliminazione e verifica dei conflitti usano le versioni SFTP, più lente. Il terminale remoto resta: è già un ssh indipendente, autenticato nel terminale.

### Distribuzione

| # | Opzione | Esito |
|---|---|---|
| 3a | pacchetto NSIS/MSI senza firma (SmartScreen avvisa, come Gatekeeper su macOS) | **subito** |
| 3b | firma con Azure Trusted Signing (circa 10 $ al mese) | più avanti, da decidere |
| 3c | aggiornamenti automatici (il plugin supporta NSIS/MSI; voci Windows in `latest.json`) | con 3a o 3b |
| 3d | winget | dopo la firma |

## Parti da adattare

| Parte | Su Windows |
|---|---|
| Askpass (`askpass.rs`) | named pipe al posto del socket Unix, stesso token. Da verificare che l'eseguibile dell'app (programma grafico) possa fare da helper e restituire la risposta a ssh; altrimenti un piccolo helper da console |
| Versione di ssh | OpenSSH ≥ 8.4 per `SSH_ASKPASS_REQUIRE`: Windows 10 ha la 8.1, Windows 11 recente la 9.5, winget la 9.8. Da verificare che l'askpass funzioni con Win32-OpenSSH |
| Processi | ogni `ssh` va lanciato con `CREATE_NO_WINDOW`, altrimenti si apre una console |
| Runtime dir (`runtime_dir.rs`) | `%LOCALAPPDATA%\glassferry`; senza socket non c'è il limite di lunghezza |
| ssh_config generato | `Include ~/.ssh/config` va bene; `/etc/ssh/ssh_config` diventa `%PROGRAMDATA%\ssh\ssh_config` |
| Pannello locale | lettere di unità e `\`: la radice è l'elenco delle unità; breadcrumb e percorsi locali nel frontend |
| Proprietario e permessi locali (`ids.rs`, `attrs.rs`) | nascosti |
| Cestino, Portachiavi, `notify` | supportati dai crate; `keyring` con la feature `windows-native` (Gestione credenziali) |
| Terminale (`terminal.rs`) | Windows Terminal (`wt.exe`), PowerShell, cmd |
| Modifica dei file | app predefinita; le regole per estensione puntano a un `.exe` |
| Archivi locali | `tar.exe` (bsdtar) di Windows 10+, che estrae anche gli zip |
| Tail locale | niente `tail`: in locale la lettura a intervalli (rivedere la scelta in [`architecture.md`](architecture.md) §18) |
| CI | job `windows-latest` in `ci.yml` e `release.yml` |

## Fasi

0. **Prova di fattibilità** (1–2 giorni) su una VM Windows 11 (ARM su Mac Apple silicon): askpass con l'ssh di Microsoft avviato da un'app grafica, named pipe, `CREATE_NO_WINDOW`, SFTP su stdin/stdout; ControlMaster con l'ssh di Git for Windows. Se l'askpass non funziona, il resto non ha senso.
1. **Solo SFTP** (2a dentro 2c): compilazione con i `cfg`, askpass su named pipe, sessione senza master, pannello locale con le unità, terminale, Gestione credenziali, CI e pacchetto NSIS (3a).
2. **Comandi shell con una connessione per comando** (2b) e master con gli ssh che lo supportano (1b, 1c).
3. **Distribuzione**: firma (3b), aggiornamenti automatici (3c), winget (3d).

## Ancora da decidere

- In quale wave (dipende da chi lo userà: il team o il pubblico).
- Requisito OpenSSH ≥ 8.4, segnalato in Impostazioni come già oggi la versione minima.
- La firma a pagamento.
