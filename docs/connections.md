# Gestione connessioni e più pannelli remoti

Analisi del 2026-10-27. Stato: tutte e quattro le fasi implementate. Voci collegate in [`roadmap.md`](roadmap.md).

## Obiettivi

- Un elenco interno di connessioni, con nome e categoria colorata. Ogni connessione è importata da `~/.ssh/config` oppure configurata nell'app.
- Conferme rinforzate sulle connessioni di categorie protette (per esempio Produzione), che mostrano i comandi che stanno per essere eseguiti.
- Toolbar in alto con "Nuovo collegamento" e "Gestisci connessioni". La lista host a sinistra sparisce.
- Il locale resta sempre a sinistra; a destra si aprono fino a 2 connessioni, con drag & drop tra tutti i pannelli.

## Modello dati

Salvato in `~/Library/Application Support/glassferry/connections.json` (Linux: `$XDG_CONFIG_HOME/glassferry/`). Supporta import ed export, ma **le password non finiscono mai nel file**.

```jsonc
{
  "version": 1,
  "categories": [
    { "id": "dev",  "name": "Sviluppo",   "color": "#2e9d5b", "protected": false },
    { "id": "test", "name": "Test",       "color": "#d9a400", "protected": false },
    { "id": "prod", "name": "Produzione", "color": "#d23c3c", "protected": true }
  ],
  "connections": [
    { "id": "7f3a…", "name": "Web produzione", "categoryId": "prod",
      "source": { "kind": "sshConfig", "alias": "prod-web" },
      "remoteDir": "/var/www", "localDir": null, "savePassword": false },
    { "id": "91bc…", "name": "Deploy staging", "categoryId": "test",
      "source": { "kind": "app", "host": "10.0.0.5", "user": "deploy", "port": 2222,
                  "identityFile": "~/.ssh/id_deploy", "proxyJump": "bastion",
                  "extraOptions": ["ServerAliveInterval 30"] },
      "remoteDir": null, "localDir": null, "savePassword": true }
  ]
}
```

- **Categorie:** di default Sviluppo, Test e Produzione. Si possono modificare (nome, colore, protetta sì/no). Una categoria non si elimina finché ha connessioni.
- **Connessioni dal config:** l'app salva solo l'alias e i propri metadati. Le impostazioni ssh restano nel tuo `~/.ssh/config` e ssh le legge come oggi.
  - Se l'alias sparisce dal config, la connessione resta ma viene segnata "alias non trovato".
- **Gli host del config compaiono solo se importati:** una connessione senza categoria non esiste.
  - Nella finestra di gestione, "Importa da ~/.ssh/config" mostra gli alias non ancora importati. Si selezionano (anche più di uno) e si assegna una categoria.
  - Al primo avvio, con l'elenco vuoto, la finestra si apre direttamente sull'import.

## Connessioni configurate nell'app: `ssh_config` generato

L'app scrive `~/Library/Application Support/glassferry/ssh_config` (mode 0600) e avvia ssh con `-F <file>` solo per queste connessioni:

```
# Generato da Glassferry: non modificare a mano.
Host gf-91bc
  HostName 10.0.0.5
  User deploy
  Port 2222
  IdentityFile ~/.ssh/id_deploy
  ProxyJump bastion
  ServerAliveInterval 30

Match all
Include ~/.ssh/config
```

- **Il principio non cambia:** l'app scrive il config, ma è sempre ssh a interpretarlo.
- **`Match all` + `Include` alla fine:** i default del tuo config (`Host *`, `IdentityAgent` di 1Password) valgono anche per queste connessioni, e `ProxyJump` può puntare a un tuo alias.
  - Verificato con `ssh -G -F`: si risolvono sia gli host dell'app sia gli alias di `~/.ssh/config`.
- **Precedenza:** in ssh vince il primo valore trovato, quindi le impostazioni della connessione hanno la precedenza sui default.
- **Riscrittura del file:** il file viene riscritto, in modo atomico, a ogni modifica.
- **Log:** mostra `ssh -F <file> gf-91bc`, incollabile nel terminale.
- **Validazione delle opzioni libere** (`extraOptions`): una per riga, nella forma `Chiave valore`. Il modulo le verifica prima del salvataggio con `ssh -G -F <file temporaneo>`. Il `ControlMaster`/`ControlPath` dell'utente resta ignorato, come oggi.
- **Esporta in `~/.ssh/config`:** possibile in futuro, ma non è in questa fase.

Campi del modulo: nome, categoria, host, utente, porta, chiave (selezione file), ProxyJump (testo, con suggerimento degli alias), cartella iniziale remota e locale, opzioni libere, "Salva password nel Portachiavi".

## Password nel Portachiavi

- **Dove:** crate `keyring` (MIT/Apache), che usa il Portachiavi su macOS e il Secret Service su Linux. Servizio `glassferry`, account `<id connessione>`.
- **Scelta per connessione** (`savePassword`). Al primo prompt `secret`, il dialogo askpass mostra "Salva nel Portachiavi". La password viene salvata solo dopo che la connessione è riuscita.
- **Risposta automatica:** alla connessione successiva l'askpass risponde da solo al **primo** prompt di password di quella connessione, senza mostrare dialoghi.
  - Se ssh richiede di nuovo (password cambiata), si mostra il dialogo con "La password salvata non è valida" e la si aggiorna. Così non si va mai in loop.
- **Esclusioni:** le passphrase delle chiavi le gestisce già l'agent (`UseKeychain` / 1Password), e OTP e conferme non si salvano mai. Si salvano solo le password.
- **Log:** riporta "risposta dal Portachiavi", mai il valore. Eliminando la connessione si rimuove anche la voce dal Portachiavi.

## Interfaccia

### Toolbar

```
[+ Nuovo collegamento  ⌘K] [Gestisci connessioni] [▥ affiancati | ▤ impilati] ········· [Pannello ⌘J]
```

- **Nuovo collegamento:** ricerca veloce sulle connessioni (nome, alias, host, categoria). Invio connette. Le connessioni sono raggruppate per categoria e l'elenco mostra il colore.
  - Se ci sono già 2 connessioni aperte, chiede quale sostituire.
- **Gestisci connessioni:** modale con elenco a sinistra (raggruppato per categoria) e dettaglio a destra.
  - Azioni: nuova, duplica, elimina, importa, esporta, gestione categorie.
  - Le connessioni dal config mostrano le impostazioni effettive (`ssh -G`) in sola lettura.
- **Affiancati / impilati:** la disposizione dei pannelli remoti, salvata. Di default affiancati.

### Pannelli

```
affiancati                                 impilati
┌─────────┬──────────┬──────────┐          ┌─────────┬──────────────────┐
│ Locale  │● prod  ⧉×│● test  ⧉×│          │ Locale  │● prod          ⧉×│
│         │┃         │┃         │          │         ├──────────────────┤
│         │┃         │┃         │          │         │● test          ⧉×│
└─────────┴──────────┴──────────┘          └─────────┴──────────────────┘
```

- **Struttura:** 2 o 3 pannelli. Il locale non si chiude e ogni pannello remoto ha la sua connessione.
- **Divisori:** trascinabili, con le larghezze salvate. La finestra ha una larghezza minima intorno ai 900 px. Le colonne nascondono permessi e data quando il pannello è stretto, come oggi.
- **Intestazione del pannello remoto:** nome della connessione, badge della categoria, bordo del colore della categoria, `⧉` "Apri nell'altro pannello" e `×` chiudi.
- **Stessa connessione in entrambi i pannelli:** le due viste condividono una sola sessione, cioè lo stesso master e lo stesso canale SFTP.
- **Pannello in basso:** host e colore su ogni voce di Trasferimenti e File aperti, con un filtro per host come nel Log.

### Destinazione dei trasferimenti

- **Drag & drop:** verso qualunque pannello. Rilasciando su una cartella si copia dentro quella cartella. Nello stesso pannello si sposta, come oggi.
- **F5 / pulsante:**
  - dal locale va al remoto usato per ultimo, con il nome sul pulsante ("Carica su prod →");
  - da un remoto va al locale;
  - ⇧F5 apre il menu delle destinazioni (locale, altro remoto).

## Trasferimenti tra pannelli remoti

| Da → a | Copia | Sposta |
|---|---|---|
| Remoto A → remoto B (server diversi) | Streaming via Mac: lettura da A e scrittura su B in pipeline, senza file su disco | Non previsto: copia, poi eliminazione esplicita |
| Stessa connessione, due cartelle | `cp -a` via shell; altrimenti estensione `copy-data` se il server la offre; altrimenti streaming via Mac | `rename` SFTP, `mv` come fallback (come "Sposta in…") |

Nota dall'implementazione: sulla stessa connessione la prima scelta è l'estensione `copy-data`, non `cp -a`. Copia sul server file per file, funziona anche sugli account solo SFTP (OpenSSH ≥ 9.0) e non apre un canale shell per ogni file. Se manca si usa `cp -p` via shell, e in ultima istanza lo streaming. La copia di una cartella su sé stessa o dentro sé stessa viene rifiutata.

Lo streaming riusa il motore attuale: il job diventa sorgente → destinazione, con `.gf-part`, conflitti, mtime, progresso e annullamento. Il trascinamento tra due remoti copia; spostare tra server diversi resta fuori, perché unisce due operazioni distruttive.

## Connessioni protette

Valgono per le connessioni delle categorie con `protected: true`. Il dialogo usa il colore della categoria, mostra nome e categoria ("**Web produzione** · PRODUZIONE") e si conferma **scrivendo il nome della connessione**.

| Operazione | Conferma rinforzata |
|---|---|
| Elimina | sempre |
| Sovrascrittura in un conflitto di trasferimento (anche "applica a tutti") | sempre |
| Sposta / rinomina | sempre |
| Upload e copia da server a server verso una connessione protetta | solo se sovrascrive; i file nuovi no |
| Salvataggio di un file in modifica | al primo caricamento di ogni file aperto, con l'opzione "non chiedere più per questo file" |

Note dall'implementazione: con un caricamento non confermato il file resta nello stato "non caricato"; "Carica sul server" nel tab File aperti chiede di nuovo la conferma. "Non chiedere più" vale per connessione e percorso, anche nelle aperture successive. Rinomina e spostamento senza protezione restano immediati, senza dialogo.

### Anteprima dei comandi

Il dialogo mostra cosa verrà eseguito, con una breve spiegazione che cambia secondo la modalità:

```
Elimina 2 elementi da Web produzione · PRODUZIONE

  Via shell (rm), definitiva e immediata, include il contenuto delle cartelle:
  $ env LC_ALL=C sh -c 'rm -rf -- "$@"' sh '/var/www/old' '/var/www/tmp.log'

Scrivi "Web produzione" per confermare: [______________]   [Annulla] [Elimina]
```

```
  Via SFTP (account senza shell), visita ricorsiva, 1.284 richieste:
  sftp> rm /var/www/old/a.txt
  sftp> rm /var/www/old/b.txt
  … altri 1.280
  sftp> rmdir /var/www/old
```

- **Un solo piano per anteprima ed esecuzione.** Il backend separa la preparazione dall'esecuzione: un comando `*_plan` (per esempio `fs_delete_plan`) restituisce il piano e un id, e l'esecuzione esegue esattamente quel piano. Così l'anteprima non può divergere da ciò che viene fatto.
  - Se tra anteprima ed esecuzione qualcosa cambia (per esempio nuovi file nella cartella), il piano viene ricalcolato e richiesto di nuovo.
- **Lunghezza:** oltre 20 righe l'anteprima si accorcia con "… altri N", e il dettaglio completo finisce nel log.
- **Anche senza protezione:** l'anteprima è disponibile nelle conferme normali, come sezione richiudibile.

## Fasi di implementazione

1. **Connessioni.**
   - `connections.json`, categorie e `ssh_config` generato.
   - Modale "Gestisci connessioni" con import dal config.
   - Toolbar con ⌘K e Portachiavi.
   - Rimozione della lista host.
   - Una connessione alla volta, come oggi.
2. **Due pannelli remoti.**
   - Più sessioni nel frontend, affiancati/impilati, divisori.
   - F5/⇧F5, sostituzione della connessione, "Apri nell'altro pannello" con sessione condivisa.
   - Host e filtri nel pannello in basso.
3. **Connessioni protette.** Piani `*_plan` con anteprima dei comandi, conferma scrivendo il nome, regole della tabella.
4. **Remoto ↔ remoto.** Streaming tra server diversi, `cp -a` / `copy-data` sulla stessa connessione.

La 3 viene prima della 4, così i trasferimenti da server a server nascono già con le protezioni.
