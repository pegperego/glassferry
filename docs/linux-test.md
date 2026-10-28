# Prova dei pacchetti Linux

Checklist per provare Glassferry su una macchina Linux vera. Le parti elencate qui funzionano in modo diverso da macOS e finora sono state solo compilate dalla CI, mai usate.

## Preparazione

**Macchina.** Una distribuzione con desktop grafico (GNOME o KDE), per esempio Ubuntu 24.04 Desktop. Su un Mac Apple silicon: VM ARM con UTM o Parallels e i pacchetti `arm64`/`aarch64`; su un PC: i pacchetti `amd64`/`x86_64`.

**Pacchetti.** Dalla release su GitHub, oppure dagli artifact di una build lanciata a mano (workflow Release → Run workflow):

```sh
sudo apt install ./Glassferry_<versione>_arm64.deb    # Debian/Ubuntu
sudo dnf install ./Glassferry-<versione>-1.aarch64.rpm # Fedora/RHEL
chmod +x Glassferry_<versione>_aarch64.AppImage && ./Glassferry_<versione>_aarch64.AppImage
```

L'AppImage richiede FUSE 2: su Ubuntu 24.04 `sudo apt install libfuse2t64`.

**Server di prova.** Un server reale, oppure il fixture del progetto dentro la VM:

```sh
sudo apt install openssh-server git   # serve il binario sshd; il fixture ne avvia una copia sua, senza privilegi
git clone https://github.com/pegperego/glassferry.git && cd glassferry
scripts/test-sshd.sh start
echo "Include $PWD/.dev-sshd/ssh_config" >> ~/.ssh/config
```

Host disponibili: `gf-local` (chiave con passphrase `segreta`, chiede la chiave dell'host al primo accesso), `gf-sftponly` (solo SFTP), `gf-slow` (20 ms di latenza, utile per annullare un trasferimento a metà).

Prima di iniziare: `ssh -V` deve essere 8.4 o più recente (serve `SSH_ASKPASS_REQUIRE`).

## Checklist

Segna l'esito di ogni voce; in caso di problemi allega la riga del log dell'app (pannello Log, o il file in `~/.local/state/glassferry/logs`).

| # | Prova | Cosa verificare | Esito |
|---|---|---|---|
| 1 | Avvio | La finestra si apre; con il `.deb`/`.rpm` l'app compare nel menu delle applicazioni con l'icona; il tema chiaro/scuro segue quello del sistema | |
| 2 | Import da `~/.ssh/config` | "Gestisci connessioni" → import: compaiono gli alias, compresi quelli degli `Include` | |
| 3 | Chiave dell'host sconosciuta | Prima connessione a `gf-local`: la richiesta di conferma appare nella finestra dell'app, non nel terminale | |
| 4 | Passphrase | La passphrase della chiave viene chiesta nell'app (askpass) e la connessione riesce | |
| 5 | Password salvata | Connessione con password e "Salva nel Portachiavi": alla connessione successiva non la chiede più. Con GNOME la voce si vede in "Password e chiavi" (Seahorse), servizio `glassferry` | |
| 6 | Senza portachiavi | In una sessione senza Secret Service (es. window manager minimale): il salvataggio fallisce con un errore nel log, l'app non si blocca | |
| 7 | Navigazione | Elenco locale e remoto, file nascosti, ordinamento, tastiera | |
| 8 | Upload e download | F5 tra i pannelli; un albero di cartelle; data di modifica conservata | |
| 9 | Trascinamento dal file manager | File trascinati da Files/Dolphin sul pannello remoto vengono caricati | |
| 10 | Trascinamento tra pannelli | Dal pannello locale a quello remoto e viceversa | |
| 11 | Cestino | Eliminare un file locale: finisce nel Cestino del file manager e si può ripristinare | |
| 12 | Modifica di un file remoto | Doppio clic su un file remoto: si apre nell'editor predefinito (`xdg-open`); a ogni salvataggio viene caricato (verificare sul server) | |
| 13 | Ripresa di un trasferimento | Download grande da `gf-slow`, annullato a metà → "Riprendi": riparte dal punto raggiunto (log: "ripreso da") | |
| 14 | Chiusura con trasferimento in corso | Chiudere l'app durante un trasferimento e riaprirla: il trasferimento compare come "Non completato" e si riprende | |
| 15 | File modificato non caricato | Modificare un file aperto, disconnettere: resta "Non connesso"; riconnettendo viene caricato | |
| 16 | Percorsi | Esistono `~/.config/glassferry/connections.json`, `~/.cache/glassferry/edit/` (con file aperti), `~/.local/state/glassferry/logs/`; socket in `$XDG_RUNTIME_DIR/gf` | |
| 17 | AppImage su una distribuzione più vecchia (facoltativo) | Si avvia anche su Ubuntu 22.04 o Debian 12 | |

Quando la checklist è completa, riporta l'esito nella voce "Pacchetti Linux" di [`roadmap.md`](roadmap.md) e apri una voce per ogni problema trovato.
