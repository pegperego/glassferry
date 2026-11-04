# Supporto per Windows

Analisi del 2026-11-03. Stato: proposte accettate; prova di fattibilità (fase 0) fatta il 2026-11-04: fattibile, con le pipe sovrapposte da creare a mano (risultati sotto). Voce collegata in [`roadmap.md`](roadmap.md).

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

## Prova di fattibilità (fase 0)

2026-11-04, VM Windows 11 Pro ARM in UTM su Mac M4 Pro, contro la fixture sshd del Mac (`GF_SSHD_LISTEN=192.168.64.1 scripts/test-sshd.sh start`, file per la VM in `.dev-sshd/vm/`).

| Prova | Esito |
|---|---|
| Versione dell'ssh di sistema | `OpenSSH_for_Windows_9.5p2, LibreSSL 3.8.2`: sopra la 8.4 richiesta |
| Askpass con l'ssh di Microsoft (`SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force`, helper `.cmd`) | ✅ la passphrase arriva dall'helper, nessuna richiesta nel terminale |
| Askpass con ssh avviato senza console visibile (`Start-Process -WindowStyle Hidden`, output rediretto) | ✅ funziona: è il modo in cui l'app avvierà ssh |
| ControlMaster con l'ssh di Microsoft | ❌ `getsockname failed: Not a socket` (come atteso) |
| ssh di Git for Windows (`OpenSSH_10.6p1`, MSYS2) | legge `~/.ssh/config` e la chiave di Windows, ma rifiuta una chiave con a capo CRLF (`invalid format`) che l'ssh di Microsoft accetta; l'askpass funziona anche lì |
| ControlMaster con l'ssh di Git for Windows | ❌ il master parte (`-O check`: `Master running`), ma ogni canale fallisce (`mux_client_request_session: read from master failed: Connection reset by peer`) e ssh ripiega in silenzio su una connessione nuova con una nuova autenticazione (`Authenticated to …` a ogni comando) |
| SFTP con l'ssh di Microsoft | ✅ `sftp gf-sftponly` interattivo, passphrase dall'askpass. Con `sftp -b` no: la modalità batch passa a ssh `BatchMode=yes`, che disattiva anche l'askpass (vale ovunque, non solo su Windows) |

Conclusioni:
- L'askpass funziona anche senza console: la modalità solo SFTP (2a) è fattibile.
- Nessun ssh disponibile su Windows ha un ControlMaster utilizzabile: con Git for Windows il master *sembra* attivo ma non viene usato. La rilevazione di 2c non può fermarsi a `-O check`: deve aprire un canale e verificare che non ci sia una seconda autenticazione (per esempio con `-o BatchMode=yes` sul canale, che fallisce invece di riautenticarsi). In pratica su Windows la base è la sessione senza master.

### Seconda parte: dall'eseguibile (2026-11-04)

Esempio Rust a parte, [`spikes/windows/`](../spikes/windows/) (il crate dell'app non compila su Windows), compilato nella VM (Rust 1.99 `aarch64-pc-windows-msvc`, Visual Studio Build Tools) e lanciato via SSH dal Mac (OpenSSH Server attivo nella VM, chiave `~/.ssh/gf-vm`). Come l'app, un solo eseguibile grafico (`windows_subsystem = "windows"`) che fa anche da helper askpass.

| Prova | Esito |
|---|---|
| `openssh-sftp-client` e Tokio su Windows ARM64 | ✅ compilano |
| Askpass su named pipe (`\\.\pipe\…`, token, `reject_remote_clients`) con l'eseguibile grafico come helper | ✅ la passphrase arriva all'app e torna a ssh |
| `CREATE_NO_WINDOW` | ✅ nessun problema (da guardare a occhio dal desktop: nessuna finestra di console) |
| stdout di ssh su file | ✅ |
| **stdout di ssh su pipe di Rust/Tokio** (e anche su pipe anonima `CreatePipe`) | ❌ **ssh si autentica, esegue il comando e poi resta bloccato**, ogni volta. Lo stderr su pipe invece va bene |
| stdout su named pipe **sovrapposta da entrambi i lati** (`FILE_FLAG_OVERLAPPED` anche sull'handle del processo figlio, come fa `sftp.exe`) | ✅ sempre, ~250 ms |
| SFTP con stdin e stdout su pipe sovrapposte da entrambi i lati | ✅ apertura 230–260 ms, `realpath`, `readdir` (209 voci), scrittura + lettura + cancellazione |
| Chiusura della sessione SFTP (`Sftp::close`) | ❌ non termina, e ssh non esce: la fine dello stdin non arriva a ssh. Da capire; intanto chiudere = terminare il processo ssh |

Conseguenze per la fase 1:
- **Le pipe verso ssh vanno create a mano**: named pipe con `FILE_FLAG_OVERLAPPED` su entrambi gli handle; il lato dell'app avvolto in `tokio::net::windows::named_pipe::NamedPipeServer`, quello del figlio ereditabile (`Stdio::from(OwnedHandle)`). `Stdio::piped()` non funziona con l'OpenSSH di Microsoft. Vale per il canale SFTP e per ogni exec di cui si legge lo stdout.
- Askpass: named pipe al posto del socket Unix, stesso schema a token; l'eseguibile dell'app può restare l'helper.
- Chiusura di una sessione senza master: terminare il processo ssh dopo `close` (con un timeout breve), finché non si capisce perché l'EOF dello stdin non arriva.

## Fasi

0. **Prova di fattibilità** (1–2 giorni) su una VM Windows 11 (ARM su Mac Apple silicon): askpass con l'ssh di Microsoft avviato da un'app grafica, named pipe, `CREATE_NO_WINDOW`, SFTP su stdin/stdout; ControlMaster con l'ssh di Git for Windows. Se l'askpass non funziona, il resto non ha senso.
1. **Solo SFTP** (2a dentro 2c): compilazione con i `cfg`, askpass su named pipe, sessione senza master, pannello locale con le unità, terminale, Gestione credenziali, CI e pacchetto NSIS (3a).
2. **Comandi shell con una connessione per comando** (2b) e master con gli ssh che lo supportano (1b, 1c).
3. **Distribuzione**: firma (3b), aggiornamenti automatici (3c), winget (3d).

## Ancora da decidere

- In quale wave (dipende da chi lo userà: il team o il pubblico).
- Requisito OpenSSH ≥ 8.4, segnalato in Impostazioni come già oggi la versione minima.
- La firma a pagamento.
