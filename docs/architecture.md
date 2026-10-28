# Architettura e decisioni

Questo documento raccoglie le scelte di fondo del progetto e il loro perché. I dettagli di implementazione (moduli, comandi, convenzioni) sono in [`CLAUDE.md`](../CLAUDE.md); gli sviluppi futuri in [`roadmap.md`](roadmap.md).

## Requisiti di partenza

- Rispettare `~/.ssh/config` al 100%: `Include`, `Match`, `ProxyJump`, `IdentityAgent` (per esempio 1Password), `RemoteCommand`, tutto come nel terminale.
- Non installare nulla sui server.
- Nessuna dipendenza closed source (niente macFUSE).
- Licenza MIT, macOS come piattaforma principale, Linux supportato.

## 1. Il binario `ssh` di sistema, non una libreria SSH

**Decisione.** Tutte le connessioni passano da `/usr/bin/ssh`. L'app non legge né interpreta il config: elenca solo gli alias `Host` per la lista, e i valori effettivi li chiede a `ssh -G <alias>`.

**Perché.** Una libreria SSH (libssh2, russh) dovrebbe reimplementare il parsing del config, `Match exec`, `ProxyJump` a catena, gli agent, `UseKeychain` di Apple, le chiavi FIDO. Non arriverebbe mai al 100%, e ogni differenza è un bug che l'utente vede come "nel terminale funziona". Con il binario di sistema il comportamento è per costruzione quello del terminale.

**Come.**
- L'app avvia un proprio ControlMaster: `ssh -M -N -f -o ControlPersist=15m -S <socket> -E <log> -- alias`. Il socket è in una cartella privata e corta (`/tmp/sfm-<uid>`, 0700), perché i path dei socket Unix sono limitati a circa 104 byte su macOS. Non riusa il `ControlPath` dell'utente, per non interferire con le sue sessioni.
- I canali successivi (SFTP, comandi) riusano il master senza riautenticarsi: `-S <socket> -o ControlMaster=no -o RemoteCommand=none -T`. `RemoteCommand=none` e `-T` servono perché con `RemoteCommand` o `RequestTTY force` nel config ssh rifiuterebbe il sottosistema o il comando.
- Ogni opzione aggiunta oltre a queste va giustificata: il principio è non sovrascrivere il config.

## 2. Niente crate `openssh`

**Decisione.** Si usa solo `openssh-sftp-client`, a cui si passano stdin/stdout di `ssh -s -- alias sftp`. Master e canali li gestisce `session.rs`, circa 150 righe.

**Perché.** Il crate `openssh` passa proprie opzioni a ssh (per esempio sulla verifica delle host key) e cambia quindi il comportamento rispetto al terminale. Gestire il master in proprio permette di controllare esattamente la riga di comando, che è anche quella mostrata nel log.

## 3. Askpass dentro l'app

**Decisione.** Il binario dell'app fa anche da `SSH_ASKPASS`, con `SSH_ASKPASS_REQUIRE=force`. Quando ssh lo avvia, si collega all'app già aperta tramite un socket Unix protetto da token, e il prompt appare come dialogo nella finestra.

**Perché.**
- Un solo binario da firmare e notarizzare.
- Prompt nella UI invece di una finestra separata.
- Funziona per tutti i tipi di richiesta di ssh: segreti (password, passphrase, OTP), conferme (chiavi FIDO, `ssh-add -c`), avvisi, e la domanda sulla host key, a cui si risponde con del testo.
- 1Password non richiede lavoro: `IdentityAgent` lo gestisce ssh e 1Password mostra la sua autorizzazione.

**Sicurezza.** Le risposte, il token e l'ambiente di ssh non vengono mai scritti nel log.

## 4. SFTP come base, shell come acceleratore

**Decisione.** Ogni funzione deve funzionare con il solo SFTP v3. Se l'account ha una shell, alla connessione una sonda (`probe`) rileva i comandi disponibili e alcune operazioni li usano.

| Operazione | Solo SFTP | Con shell |
|---|---|---|
| Eliminazione ricorsiva | Visita dell'albero, una richiesta per file | `rm -rf` |
| Spostamento tra filesystem diversi | `rename` fallisce | `mv` |
| Controllo conflitti nella modifica | Dimensione e mtime (1 s) | Anche `sha256sum` |

**Perché ibrido e non solo shell.**
- Gli account solo SFTP (`ForceCommand internal-sftp`, chroot, hosting, NAS) non permettono comandi.
- La login shell può essere bash, zsh, fish, csh o BusyBox, con opzioni diverse.
- I comandi danno errori in testo libero e niente progresso.

**Sicurezza del quoting.** I comandi remoti si costruiscono solo con `sh_command(script, args)`: `env LC_ALL=C sh -c '<script>' sh 'arg1' 'arg2'`, con i path come parametri posizionali. Non si concatenano mai stringhe. `LC_ALL=C` rende gli errori prevedibili.

## 5. Limiti di SFTP v3 e come li gestiamo

| Limite | Soluzione |
|---|---|
| `rename` fallisce se la destinazione esiste | Estensione `posix-rename@openssh.com` quando il server la supporta; le operazioni sui file non sovrascrivono mai |
| Nessuna copia lato server | Fuori MVP (in roadmap: `cp -a` o `copy-data`) |
| `rename` tra filesystem diversi fallisce | Fallback su `mv` se c'è la shell |
| mtime con risoluzione di 1 secondo | Nei conflitti di modifica si aggiunge `sha256sum` quando disponibile |
| Una richiesta alla volta è lenta con latenza | 16 richieste da 256 KiB in volo per file, 3 file in parallelo |

## 6. Trasferimenti

- Ogni file si scrive come `<nome>.sfm-part` e si rinomina a fine copia, così la destinazione non contiene mai un file troncato.
- Si conservano mtime e, solo per i file nuovi, i permessi.
- I link a cartelle non vengono seguiti, per evitare cicli.

**Benchmark** (fixture locale con 20 ms di latenza per direzione, 40 ms di RTT):

| Modalità | Velocità |
|---|---|
| Lettura sequenziale | ~5 MiB/s |
| Pipeline (16 × 256 KiB) | ~17 MiB/s |
| `sftp` di OpenSSH sullo stesso percorso | ~22 MiB/s |

## 7. Modifica dei file remoti

- Il file viene scaricato in `~/Library/Caches/sshfilemanager/edit/<hash>/<nome>`, così l'editor mostra il nome giusto, e aperto con l'app predefinita.
- Si osserva la **cartella**, non il file: molti editor salvano in modo atomico, scrivendo un file nuovo e rinominandolo. Debounce di 500 ms, e si carica solo se il contenuto (SHA-256) è cambiato.
- **Scrittura diretta** sul file remoto (troncamento e scrittura) invece di temporaneo + rename. Mantiene proprietario, permessi e ACL; il prezzo è un file parziale se la connessione cade a metà. Decisione presa all'inizio del progetto.
- Prima di caricare si confronta il file remoto con la versione aperta. Se è cambiato, si chiede: sovrascrivi, usa la versione del server (la propria resta come "(mia versione)"), oppure decidi dopo.

## 8. Eliminazione

- **Locale:** nel Cestino (crate `trash`, metodo NSFileManager, senza richiesta di permessi del Finder).
- **Remota:** definitiva, con conferma. Rifiuta `/` e la home.

## 9. Stack dell'interfaccia

- **Tauri 2**: binario piccolo, backend Rust, nessun Chromium incluso.
- **React 19 + TypeScript strict.** Scelto perché è il framework su cui un agente di coding lavora con più affidabilità; Svelte era l'alternativa più leggera.
- La logica pura (ordinamento, selezione, log, drag & drop) sta in moduli TypeScript testati con vitest, non nei componenti.

## 10. Connessioni dell'app

L'app tiene un proprio elenco di connessioni, con nome e categoria colorata. Il progetto completo è in [`connections.md`](connections.md).

- **Alias di `~/.ssh/config`:** l'app salva solo l'alias e i propri metadati, e ssh legge il tuo config come sempre.
- **Connessioni configurate nell'app:** l'app scrive un `ssh_config` suo, con un blocco `Host sfm-<id>` per ognuna, e avvia ssh con `-F`.
  - Il file termina con `Match all`, `Include ~/.ssh/config` e `Include /etc/ssh/ssh_config`. Così valgono i tuoi default (1Password, `Host *`), ProxyJump può puntare ai tuoi alias, e il file di sistema viene letto anche se `-F` lo escluderebbe.
  - Il principio non cambia: l'app scrive il config, ma è sempre ssh a interpretarlo.
  - Verificato sul server di test, compreso ProxyJump tra due connessioni dell'app: ssh passa `-F` anche al processo del jump.
  - Le opzioni libere passano da una validazione (`Host`, `Match` e `Include` sono rifiutate) e da `ssh -G` su un file temporaneo, così un'opzione sbagliata la segnala ssh.
- **Password:** solo nel Portachiavi, una voce per connessione. L'askpass le usa una volta sola per connessione, quindi una password cambiata fa comparire il dialogo invece di andare in loop.

## 11. Anteprima ed esecuzione: un solo piano

Le operazioni distruttive passano da un piano (`plan.rs`): il backend decide come eseguirle (Cestino, `rm -rf` via shell, elenco delle richieste SFTP, rename) e il dialogo mostra esattamente quei comandi. All'esecuzione si esegue il piano mostrato, non uno ricalcolato.

**Perché.** Un'anteprima costruita dal frontend potrebbe divergere da quello che fa il backend. Con il piano, l'anteprima non può mostrare una cosa diversa da quella eseguita. Per l'eliminazione via SFTP vale anche il contrario: i file comparsi dopo l'anteprima non vengono eliminati, perché la `rmdir` della loro cartella fallisce (verificato sul server di test).

## 12. Copie tra due pannelli remoti

**Decisione.** Due strade diverse:
- **Stessa connessione:** copia sul server, con l'estensione SFTP `copy-data` se c'è, altrimenti con `cp -p` via shell.
- **Server diversi:** streaming attraverso l'app, con letture e scritture in pipeline e nessun file temporaneo sul disco locale.

**Perché.** Sulla stessa connessione far passare i dati dal Mac sarebbe un viaggio inutile. `copy-data` funziona anche senza shell e conserva l'avanzamento file per file. Tra server diversi non si può installare nulla, quindi l'unica strada è passare dal client, come `scp -3`.

Lo spostamento tra server diversi non è previsto, perché unirebbe due operazioni distruttive: si copia, e poi si elimina a parte.

**Verifica sul server di test.** 5 MiB in 5 file: copia identica (`diff -r`) con tutte e quattro le combinazioni (`copy-data`, `cp -p`, streaming sulla stessa sessione, streaming tra due sessioni), e il conflitto viene chiesto una volta sola.

## 13. Distribuzione

Fuori dal Mac App Store: la sandbox impedirebbe di avviare `ssh` e di leggere `~/.ssh`. Build firmata e notarizzata, arm64 e x64 (in roadmap).

Un'app aperta dal Finder non legge `.zshrc`: chi esporta `SSH_AUTH_SOCK` dalla shell invece di usare `IdentityAgent` non avrà quell'agent. Il `SSH_AUTH_SOCK` di launchd invece è disponibile.
